use crate::model::*;
use crate::openapi::{validate_tape_semantics, OpenApiRegistry};
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratedMockResult {
    pub data: Value,
    pub validated: bool,
    pub attempts: usize,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratedTapeResult {
    pub tape: Tape,
    pub validated: bool,
    pub attempts: usize,
    pub warnings: Vec<String>,
}

async fn call_chat_completions(
    cfg: &LlmConfig,
    messages: &[Value],
) -> Result<String> {
    let url = format!("{}/chat/completions", cfg.api_base.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()?;

    let req_body = json!({
        "model": cfg.model,
        "temperature": cfg.temperature,
        "response_format": { "type": "json_object" },
        "messages": messages,
    });

    let mut req = client.post(&url).json(&req_body);
    if !cfg.api_key.is_empty() {
        req = req.bearer_auth(&cfg.api_key);
    }

    let res = req.send().await.context("LLM API request failed")?;
    let status = res.status();
    let text = res.text().await?;
    if !status.is_success() {
        return Err(anyhow!("LLM API returned HTTP {status}: {text}"));
    }
    let parsed: Value = serde_json::from_str(&text).context("invalid LLM API JSON envelope")?;
    let content = parsed
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("LLM response missing choices[0].message.content"))?;
    Ok(content.to_string())
}

/// Extract a JSON value from LLM output, stripping optional ```json ... ``` markdown fences.
pub fn parse_llm_json(raw: &str) -> Result<Value> {
    let trimmed = raw.trim();
    if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
        return Ok(v);
    }
    if let Some(start) = trimmed.find("```") {
        let after = &trimmed[start + 3..];
        let body = after
            .strip_prefix("json")
            .or_else(|| after.strip_prefix("JSON"))
            .unwrap_or(after);
        if let Some(end) = body.rfind("```") {
            let candidate = body[..end].trim();
            return serde_json::from_str(candidate)
                .context("failed to parse fenced JSON from LLM output");
        }
    }
    Err(anyhow!("LLM output did not contain valid JSON"))
}

/// Generate a realistic HTTP Mock response body using an OpenAI-compatible LLM,
/// constrained by the OpenAPI schema and validated with an automatic repair loop.
pub async fn generate_http_mock(
    cfg: &LlmConfig,
    registry: &OpenApiRegistry,
    method: &str,
    path: &str,
    prompt: &str,
    directory: &str,
) -> Result<GeneratedMockResult> {
    let op = registry.find_operation(method, path);
    let schema = op.as_ref().and_then(|o| o.response_schema.clone());
    let bundled = schema.as_ref().map(|s| registry.bundle_schema(s));

    let system = format!(
        "You generate valid JSON mock responses for OpenCode C/S HTTP APIs.\n\
         Target endpoint: {} {}\n\
         Active directory: {}\n\
         Return ONLY raw JSON (no prose, no markdown fences). If the schema expects a JSON array, wrap or return valid JSON matching the schema.",
        method, path, directory
    );

    let user_msg = match &bundled {
        Some(s) => format!(
            "User scenario:\n{}\n\nTarget JSON Schema:\n{}",
            prompt,
            serde_json::to_string_pretty(s)?
        ),
        None => format!("User scenario:\n{}\n\nReturn a realistic OpenCode JSON response.", prompt),
    };

    let mut messages = vec![
        json!({ "role": "system", "content": system }),
        json!({ "role": "user", "content": user_msg }),
    ];

    let mut warnings = Vec::new();
    let max_attempts = 3usize;
    let mut last_value = Value::Null;

    for attempt in 1..=max_attempts {
        let raw = call_chat_completions(cfg, &messages).await?;
        let parsed = match parse_llm_json(&raw) {
            Ok(v) => v,
            Err(err) => {
                warnings.push(format!("attempt {attempt}: {err}"));
                messages.push(json!({ "role": "assistant", "content": raw }));
                messages.push(json!({
                    "role": "user",
                    "content": format!("Invalid JSON ({err}). Return ONLY valid JSON.")
                }));
                continue;
            }
        };
        last_value = parsed.clone();

        if let Some(ref s) = schema {
            match registry.validate(s, &parsed) {
                Ok(()) => {
                    return Ok(GeneratedMockResult {
                        data: parsed,
                        validated: true,
                        attempts: attempt,
                        warnings,
                    });
                }
                Err(errs) => {
                    warnings.extend(errs.iter().map(|e| format!("attempt {attempt}: {e}")));
                    if attempt < max_attempts {
                        messages.push(json!({ "role": "assistant", "content": raw }));
                        messages.push(json!({
                            "role": "user",
                            "content": format!(
                                "JSON Schema validation failed with:\n- {}\nFix all errors and return ONLY the corrected JSON.",
                                errs.join("\n- ")
                            )
                        }));
                    }
                }
            }
        } else {
            return Ok(GeneratedMockResult {
                data: parsed,
                validated: true,
                attempts: attempt,
                warnings,
            });
        }
    }

    Ok(GeneratedMockResult {
        data: last_value,
        validated: false,
        attempts: max_attempts,
        warnings,
    })
}

/// Generate a complete multi-frame OpenCode SSE Session Tape from a natural-language scenario,
/// validating event ordering invariants (`message.part.updated` before `message.part.delta`, etc.)
/// with an automatic self-repair loop.
pub async fn generate_sse_scenario(
    cfg: &LlmConfig,
    registry: &OpenApiRegistry,
    prompt: &str,
    directory: &str,
    tape_name: Option<String>,
) -> Result<GeneratedTapeResult> {
    let event_types: Vec<String> = registry
        .event_schemas()
        .into_iter()
        .map(|e| e.event_type)
        .collect();

    let system = format!(
        "You generate realistic OpenCode SSE Session Replay Tapes as JSON.\n\
         Return a JSON object with shape:\n\
         {{\n\
           \"name\": \"short title\",\n\
           \"description\": \"summary\",\n\
           \"session_id\": \"ses_ai_001\",\n\
           \"events\": [\n\
             {{ \"offset_ms\": 0, \"directory\": \"{dir}\", \"payload\": {{ \"type\": \"session.created\", \"properties\": {{ \"info\": {{ ... }} }} }} }},\n\
             ...\n\
           ]\n\
         }}\n\
         CRITICAL INVARIANTS:\n\
         1. offset_ms MUST be non-decreasing integers starting at 0.\n\
         2. Start with session.created and session.status(busy), create user + assistant message.updated events.\n\
         3. Every message.part.delta MUST be preceded by a message.part.updated for the SAME (messageID, partID).\n\
         4. End with session.status(idle) and session.idle.\n\
         5. Supported event types: {types}\n\
         Return ONLY raw JSON.",
        dir = directory,
        types = event_types.join(", ")
    );

    let mut messages = vec![
        json!({ "role": "system", "content": system }),
        json!({ "role": "user", "content": prompt }),
    ];

    let mut warnings = Vec::new();
    let max_attempts = 3usize;
    let mut last_tape: Option<Tape> = None;

    for attempt in 1..=max_attempts {
        let raw = call_chat_completions(cfg, &messages).await?;
        let parsed = match parse_llm_json(&raw) {
            Ok(v) => v,
            Err(err) => {
                warnings.push(format!("attempt {attempt}: {err}"));
                messages.push(json!({ "role": "assistant", "content": raw }));
                messages.push(json!({
                    "role": "user",
                    "content": format!("Invalid JSON ({err}). Return ONLY valid JSON.")
                }));
                continue;
            }
        };

        let events_val = parsed
            .get("events")
            .cloned()
            .unwrap_or_else(|| Value::Array(Vec::new()));
        let events: Vec<TapeEvent> = match serde_json::from_value(events_val) {
            Ok(ev) => ev,
            Err(err) => {
                warnings.push(format!("attempt {attempt}: invalid events array: {err}"));
                messages.push(json!({ "role": "assistant", "content": raw }));
                messages.push(json!({
                    "role": "user",
                    "content": format!("Each item in events must have {{ offset_ms, directory, payload }}. Error: {err}")
                }));
                continue;
            }
        };

        let session_id = parsed
            .get("session_id")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .or_else(|| {
                events
                    .iter()
                    .find_map(|e| extract_event_ids(&e.payload).0)
            });

        let tape = Tape {
            id: format!("tape_ai_{}", &uuid::Uuid::new_v4().simple().to_string()[..8]),
            name: tape_name.clone().unwrap_or_else(|| {
                parsed
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("AI Generated Scenario")
                    .to_string()
            }),
            description: parsed
                .get("description")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            directory: directory.to_string(),
            session_id,
            snapshot: TapeSnapshot::default(),
            events,
            created_at: now_ms(),
        };

        last_tape = Some(tape.clone());
        match validate_tape_semantics(&tape) {
            Ok(()) => {
                return Ok(GeneratedTapeResult {
                    tape,
                    validated: true,
                    attempts: attempt,
                    warnings,
                });
            }
            Err(errs) => {
                warnings.extend(errs.iter().map(|e| format!("attempt {attempt}: {e}")));
                if attempt < max_attempts {
                    messages.push(json!({ "role": "assistant", "content": raw }));
                    messages.push(json!({
                        "role": "user",
                        "content": format!(
                            "Tape semantic validation failed:\n- {}\nFix these issues and return ONLY the corrected JSON.",
                            errs.join("\n- ")
                        )
                    }));
                }
            }
        }
    }

    let tape = last_tape.ok_or_else(|| anyhow!("LLM failed to produce a parseable tape"))?;
    Ok(GeneratedTapeResult {
        tape,
        validated: false,
        attempts: max_attempts,
        warnings,
    })
}

/// Mutate a real captured HTTP response or SSE payload for edge-case / chaos testing.
pub async fn mutate_payload(
    cfg: &LlmConfig,
    payload: &Value,
    instruction: &str,
) -> Result<Value> {
    let messages = vec![
        json!({
            "role": "system",
            "content": "You mutate real OpenCode JSON payloads to create realistic edge-case or fault-injection test variants. Return ONLY raw JSON."
        }),
        json!({
            "role": "user",
            "content": format!(
                "Mutation instruction: {}\n\nOriginal JSON:\n{}",
                instruction,
                serde_json::to_string_pretty(payload)?
            )
        }),
    ];
    let raw = call_chat_completions(cfg, &messages).await?;
    parse_llm_json(&raw)
}
