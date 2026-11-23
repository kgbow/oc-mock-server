use crate::model::*;
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "record", rename_all = "snake_case")]
enum TapeJsonlLine {
    Meta {
        id: String,
        name: String,
        description: Option<String>,
        directory: String,
        session_id: Option<String>,
        created_at: i64,
        snapshot: TapeSnapshot,
    },
    Event(TapeEvent),
}

/// Export a Tape to portable `.tape.jsonl` format (line 1 = metadata + snapshot, remaining lines = events).
pub fn export_tape_jsonl(tape: &Tape) -> Result<String> {
    let mut lines = Vec::with_capacity(tape.events.len() + 1);
    let meta = TapeJsonlLine::Meta {
        id: tape.id.clone(),
        name: tape.name.clone(),
        description: tape.description.clone(),
        directory: tape.directory.clone(),
        session_id: tape.session_id.clone(),
        created_at: tape.created_at,
        snapshot: tape.snapshot.clone(),
    };
    lines.push(serde_json::to_string(&meta)?);
    for ev in &tape.events {
        lines.push(serde_json::to_string(&TapeJsonlLine::Event(ev.clone()))?);
    }
    Ok(lines.join("\n") + "\n")
}

/// Import a Tape from `.tape.jsonl` (or a single JSON object matching `Tape`).
pub fn import_tape_jsonl(raw: &str) -> Result<Tape> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("empty tape payload"));
    }

    // Support direct JSON object import too
    if trimmed.starts_with('{') && !trimmed.contains('\n') {
        if let Ok(tape) = serde_json::from_str::<Tape>(trimmed) {
            return Ok(tape);
        }
    } else if let Ok(tape) = serde_json::from_str::<Tape>(trimmed) {
        return Ok(tape);
    }

    let mut meta: Option<(String, String, Option<String>, String, Option<String>, i64, TapeSnapshot)> =
        None;
    let mut events = Vec::new();

    for (idx, line) in trimmed.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parsed: TapeJsonlLine = serde_json::from_str(line)
            .with_context(|| format!("invalid tape JSONL on line {}", idx + 1))?;
        match parsed {
            TapeJsonlLine::Meta {
                id,
                name,
                description,
                directory,
                session_id,
                created_at,
                snapshot,
            } => {
                meta = Some((
                    id,
                    name,
                    description,
                    directory,
                    session_id,
                    created_at,
                    snapshot,
                ));
            }
            TapeJsonlLine::Event(ev) => events.push(ev),
        }
    }

    let (id, name, description, directory, session_id, created_at, snapshot) =
        meta.ok_or_else(|| anyhow!("tape JSONL missing meta header line"))?;

    Ok(Tape {
        id,
        name,
        description,
        directory,
        session_id,
        snapshot,
        events,
        created_at,
    })
}

/// Built-in realistic OpenCode demo tape showcasing thinking, tool call, permission prompt,
/// streaming deltas, file diff, todo updates, and idle completion.
pub fn builtin_demo_tape(directory: &str) -> Tape {
    let ts = now_ms();
    let sid = "ses_demo_replay_001";
    let user_mid = "msg_demo_user_001";
    let asst_mid = "msg_demo_asst_001";
    let user_pid = "prt_demo_user_001";
    let reason_pid = "prt_demo_reason_001";
    let tool_pid = "prt_demo_tool_001";
    let text_pid = "prt_demo_text_001";

    let session_info = json!({
        "id": sid,
        "slug": "demo-sse-replay",
        "projectID": "proj_replay_shadow",
        "directory": directory,
        "title": "[Demo] Fix SSE heartbeat & add replay inspector",
        "version": "1.0.0",
        "time": { "created": ts, "updated": ts }
    });

    let events = vec![
        TapeEvent {
            offset_ms: 0,
            directory: directory.to_string(),
            payload: json!({
                "type": "session.created",
                "properties": { "info": session_info }
            }),
        },
        TapeEvent {
            offset_ms: 150,
            directory: directory.to_string(),
            payload: json!({
                "type": "session.status",
                "properties": { "sessionID": sid, "status": { "type": "busy" } }
            }),
        },
        TapeEvent {
            offset_ms: 250,
            directory: directory.to_string(),
            payload: json!({
                "type": "message.updated",
                "properties": {
                    "info": {
                        "id": user_mid,
                        "sessionID": sid,
                        "role": "user",
                        "time": { "created": ts + 250 },
                        "agent": "build",
                        "model": { "providerID": "anthropic", "modelID": "claude-sonnet-4" }
                    }
                }
            }),
        },
        TapeEvent {
            offset_ms: 320,
            directory: directory.to_string(),
            payload: json!({
                "type": "message.part.updated",
                "properties": {
                    "part": {
                        "id": user_pid,
                        "sessionID": sid,
                        "messageID": user_mid,
                        "type": "text",
                        "text": "Investigate why SSE clients reconnect when the proxy pauses event streaming."
                    }
                }
            }),
        },
        TapeEvent {
            offset_ms: 500,
            directory: directory.to_string(),
            payload: json!({
                "type": "message.updated",
                "properties": {
                    "info": {
                        "id": asst_mid,
                        "sessionID": sid,
                        "role": "assistant",
                        "parentID": user_mid,
                        "modelID": "claude-sonnet-4",
                        "providerID": "anthropic",
                        "mode": "build",
                        "agent": "build",
                        "path": { "cwd": directory, "root": directory },
                        "cost": 0.0042,
                        "tokens": { "input": 1240, "output": 380, "reasoning": 95, "cache": { "read": 0, "write": 0 } },
                        "time": { "created": ts + 500 }
                    }
                }
            }),
        },
        TapeEvent {
            offset_ms: 650,
            directory: directory.to_string(),
            payload: json!({
                "type": "message.part.updated",
                "properties": {
                    "part": {
                        "id": reason_pid,
                        "sessionID": sid,
                        "messageID": asst_mid,
                        "type": "reasoning",
                        "text": "",
                        "time": { "start": ts + 650 }
                    }
                }
            }),
        },
        TapeEvent {
            offset_ms: 820,
            directory: directory.to_string(),
            payload: json!({
                "type": "message.part.delta",
                "properties": {
                    "sessionID": sid,
                    "messageID": asst_mid,
                    "partID": reason_pid,
                    "field": "text",
                    "delta": "Checking client heartbeat timeout in global-sdk.tsx... "
                }
            }),
        },
        TapeEvent {
            offset_ms: 1050,
            directory: directory.to_string(),
            payload: json!({
                "type": "message.part.delta",
                "properties": {
                    "sessionID": sid,
                    "messageID": asst_mid,
                    "partID": reason_pid,
                    "field": "text",
                    "delta": "HEARTBEAT_TIMEOUT_MS is 15000ms, so proxy must inject server.heartbeat every 9s."
                }
            }),
        },
        TapeEvent {
            offset_ms: 1300,
            directory: directory.to_string(),
            payload: json!({
                "type": "todo.updated",
                "properties": {
                    "sessionID": sid,
                    "todos": [
                        { "id": "todo_1", "content": "Inspect global-sdk heartbeat watchdog", "status": "completed", "priority": "high" },
                        { "id": "todo_2", "content": "Add independent heartbeat task to Rust SSE hub", "status": "in_progress", "priority": "high" }
                    ]
                }
            }),
        },
        TapeEvent {
            offset_ms: 1550,
            directory: directory.to_string(),
            payload: json!({
                "type": "message.part.updated",
                "properties": {
                    "part": {
                        "id": tool_pid,
                        "sessionID": sid,
                        "messageID": asst_mid,
                        "type": "tool",
                        "callID": "call_demo_rg_01",
                        "tool": "bash",
                        "state": {
                            "status": "running",
                            "input": { "command": "rg -n HEARTBEAT_TIMEOUT_MS packages/app/src" },
                            "title": "Search heartbeat timeout",
                            "time": { "start": ts + 1550 }
                        }
                    }
                }
            }),
        },
        TapeEvent {
            offset_ms: 2150,
            directory: directory.to_string(),
            payload: json!({
                "type": "message.part.updated",
                "properties": {
                    "part": {
                        "id": tool_pid,
                        "sessionID": sid,
                        "messageID": asst_mid,
                        "type": "tool",
                        "callID": "call_demo_rg_01",
                        "tool": "bash",
                        "state": {
                            "status": "completed",
                            "input": { "command": "rg -n HEARTBEAT_TIMEOUT_MS packages/app/src" },
                            "output": "packages/app/src/context/global-sdk.tsx:108:const HEARTBEAT_TIMEOUT_MS = 15_000",
                            "title": "Search heartbeat timeout",
                            "metadata": {},
                            "time": { "start": ts + 1550, "end": ts + 2150 }
                        }
                    }
                }
            }),
        },
        TapeEvent {
            offset_ms: 2450,
            directory: directory.to_string(),
            payload: json!({
                "type": "message.part.updated",
                "properties": {
                    "part": {
                        "id": text_pid,
                        "sessionID": sid,
                        "messageID": asst_mid,
                        "type": "text",
                        "text": ""
                    }
                }
            }),
        },
        TapeEvent {
            offset_ms: 2600,
            directory: directory.to_string(),
            payload: json!({
                "type": "message.part.delta",
                "properties": {
                    "sessionID": sid,
                    "messageID": asst_mid,
                    "partID": text_pid,
                    "field": "text",
                    "delta": "Found the root cause! "
                }
            }),
        },
        TapeEvent {
            offset_ms: 2850,
            directory: directory.to_string(),
            payload: json!({
                "type": "message.part.delta",
                "properties": {
                    "sessionID": sid,
                    "messageID": asst_mid,
                    "partID": text_pid,
                    "field": "text",
                    "delta": "`GlobalSDKProvider` aborts and reconnects after 15s without any SSE frame. "
                }
            }),
        },
        TapeEvent {
            offset_ms: 3150,
            directory: directory.to_string(),
            payload: json!({
                "type": "message.part.delta",
                "properties": {
                    "sessionID": sid,
                    "messageID": asst_mid,
                    "partID": text_pid,
                    "field": "text",
                    "delta": "Injecting an independent `server.heartbeat` every 9s in the Rust SSE hub keeps paused/stepped replays rock solid."
                }
            }),
        },
        TapeEvent {
            offset_ms: 3450,
            directory: directory.to_string(),
            payload: json!({
                "type": "todo.updated",
                "properties": {
                    "sessionID": sid,
                    "todos": [
                        { "id": "todo_1", "content": "Inspect global-sdk heartbeat watchdog", "status": "completed", "priority": "high" },
                        { "id": "todo_2", "content": "Add independent heartbeat task to Rust SSE hub", "status": "completed", "priority": "high" }
                    ]
                }
            }),
        },
        TapeEvent {
            offset_ms: 3650,
            directory: directory.to_string(),
            payload: json!({
                "type": "session.diff",
                "properties": {
                    "sessionID": sid,
                    "diff": [
                        {
                            "file": "crates/oc-proxy/src/sse.rs",
                            "before": "// passthrough only\n",
                            "after": "// independent 9s heartbeat keepalive\n",
                            "additions": 12,
                            "deletions": 2
                        }
                    ]
                }
            }),
        },
        TapeEvent {
            offset_ms: 3850,
            directory: directory.to_string(),
            payload: json!({
                "type": "session.status",
                "properties": { "sessionID": sid, "status": { "type": "idle" } }
            }),
        },
        TapeEvent {
            offset_ms: 3900,
            directory: directory.to_string(),
            payload: json!({
                "type": "session.idle",
                "properties": { "sessionID": sid }
            }),
        },
    ];

    Tape {
        id: "tape_demo_heartbeat".to_string(),
        name: "Demo: Heartbeat Diagnosis & Streaming Replay".to_string(),
        description: Some(
            "Built-in OpenCode session tape with reasoning deltas, bash tool execution, todo updates, diff, and idle completion."
                .to_string(),
        ),
        directory: directory.to_string(),
        session_id: Some(sid.to_string()),
        snapshot: TapeSnapshot::default(),
        events,
        created_at: ts,
    }
}
