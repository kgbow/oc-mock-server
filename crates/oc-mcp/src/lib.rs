use anyhow::Result;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use oc_core::anomaly::{find_anomalies, AnomalyOptions};
use oc_core::storage::fold_sse_events;
use oc_core::*;
use oc_proxy::{replay, AppState};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::convert::Infallible;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const PROTOCOL_VERSION: &str = "2025-03-26";

#[derive(Clone)]
pub enum McpBackend {
    Live(Arc<AppState>),
    Standalone {
        storage: Storage,
        control_url: Option<String>,
        http: reqwest::Client,
    },
}

impl McpBackend {
    pub fn standalone(storage: Storage, control_url: Option<String>) -> Self {
        Self::Standalone {
            storage,
            control_url,
            http: reqwest::Client::new(),
        }
    }

    fn storage(&self) -> &Storage {
        match self {
            Self::Live(s) => &s.storage,
            Self::Standalone { storage, .. } => storage,
        }
    }
}

#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    #[allow(dead_code)]
    jsonrpc: Option<String>,
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Debug, Serialize)]
struct JsonRpcResponse {
    jsonrpc: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
struct JsonRpcError {
    code: i64,
    message: String,
}

impl JsonRpcResponse {
    fn ok(id: Option<Value>, result: Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        }
    }

    fn err(id: Option<Value>, code: i64, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
            }),
        }
    }
}

/// Mount Streamable HTTP MCP endpoints (`/mcp`) onto an Axum Router backed by live `AppState`.
pub fn mcp_http_router(state: Arc<AppState>) -> Router {
    let backend = Arc::new(McpBackend::Live(state));
    Router::new()
        .route(
            "/mcp",
            get(handle_mcp_get)
                .post(handle_mcp_post)
                .delete(handle_mcp_delete),
        )
        .layer(DefaultBodyLimit::disable())
        .with_state(backend)
}

async fn handle_mcp_get() -> Sse<impl futures_util::Stream<Item = Result<SseEvent, Infallible>>> {
    let stream = futures_util::stream::pending::<Result<SseEvent, Infallible>>();
    Sse::new(stream).keep_alive(KeepAlive::default())
}

async fn handle_mcp_delete() -> StatusCode {
    StatusCode::NO_CONTENT
}

async fn handle_mcp_post(
    State(backend): State<Arc<McpBackend>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let session_id = headers
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    if let Value::Array(batch) = body {
        let mut responses = Vec::new();
        for item in batch {
            if let Ok(req) = serde_json::from_value::<JsonRpcRequest>(item) {
                if let Some(resp) = dispatch_rpc(&backend, req).await {
                    responses.push(resp);
                }
            }
        }
        if responses.is_empty() {
            return StatusCode::ACCEPTED.into_response();
        }
        let mut res = Json(responses).into_response();
        if let Ok(hv) = HeaderValue::from_str(&session_id) {
            res.headers_mut().insert("mcp-session-id", hv);
        }
        return res;
    }

    let req: JsonRpcRequest = match serde_json::from_value(body) {
        Ok(r) => r,
        Err(err) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(JsonRpcResponse::err(None, -32700, format!("parse error: {err}"))),
            )
                .into_response();
        }
    };

    let is_init = req.method == "initialize";
    match dispatch_rpc(&backend, req).await {
        Some(resp) => {
            let mut res = (StatusCode::OK, Json(resp)).into_response();
            if is_init || headers.get("mcp-session-id").is_some() {
                if let Ok(hv) = HeaderValue::from_str(&session_id) {
                    res.headers_mut().insert("mcp-session-id", hv);
                }
            }
            res.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            );
            res
        }
        None => StatusCode::ACCEPTED.into_response(),
    }
}

/// Run newline-delimited JSON-RPC 2.0 MCP Server over stdio (`oc-mock-server mcp`).
pub async fn run_stdio_mcp(backend: McpBackend) -> Result<()> {
    let stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let mut reader = BufReader::new(stdin).lines();

    while let Some(line) = reader.next_line().await? {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let req: JsonRpcRequest = match serde_json::from_str(trimmed) {
            Ok(r) => r,
            Err(err) => {
                let err_resp = JsonRpcResponse::err(None, -32700, format!("parse error: {err}"));
                let out = serde_json::to_string(&err_resp)?;
                stdout.write_all(out.as_bytes()).await?;
                stdout.write_all(b"\n").await?;
                stdout.flush().await?;
                continue;
            }
        };

        if let Some(resp) = dispatch_rpc(&backend, req).await {
            let out = serde_json::to_string(&resp)?;
            stdout.write_all(out.as_bytes()).await?;
            stdout.write_all(b"\n").await?;
            stdout.flush().await?;
        }
    }
    Ok(())
}

async fn dispatch_rpc(backend: &McpBackend, req: JsonRpcRequest) -> Option<JsonRpcResponse> {
    // Notifications have no `id` and expect no JSON-RPC response
    let is_notification = req.id.is_none() || req.method.starts_with("notifications/");

    match req.method.as_str() {
        "initialize" => Some(JsonRpcResponse::ok(
            req.id,
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {
                    "tools": { "listChanged": false }
                },
                "serverInfo": {
                    "name": "oc-mock-server",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }),
        )),
        "notifications/initialized" | "notifications/cancelled" => None,
        "ping" => Some(JsonRpcResponse::ok(req.id, json!({}))),
        "tools/list" => Some(JsonRpcResponse::ok(req.id, json!({ "tools": tool_definitions() }))),
        "tools/call" => {
            let name = req
                .params
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("");
            let args = req
                .params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));

            match call_tool(backend, name, args).await {
                Ok(val) => {
                    let text = serde_json::to_string_pretty(&val).unwrap_or_else(|_| val.to_string());
                    Some(JsonRpcResponse::ok(
                        req.id,
                        json!({
                            "content": [{ "type": "text", "text": text }],
                            "isError": false
                        }),
                    ))
                }
                Err(err) => Some(JsonRpcResponse::ok(
                    req.id,
                    json!({
                        "content": [{ "type": "text", "text": format!("Tool error: {err}") }],
                        "isError": true
                    }),
                )),
            }
        }
        _ => {
            if is_notification {
                None
            } else {
                Some(JsonRpcResponse::err(
                    req.id,
                    -32601,
                    format!("method not found: {}", req.method),
                ))
            }
        }
    }
}

pub fn tool_definitions() -> Value {
    json!([
        {
            "name": "list_sessions",
            "description": "List captured OpenCode sessions from proxy traffic with HTTP/SSE counts, error counts, and directory metadata. Pass exclude_session_id to ignore the current debugging session.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "exclude_session_id": { "type": "string", "description": "Optional session ID to exclude (e.g. your own active debugging session)" },
                    "include_ignored": { "type": "boolean", "default": false }
                }
            }
        },
        {
            "name": "get_session_timeline",
            "description": "Return a unified chronological HTTP + SSE timeline for a session. By default folds consecutive message.part.delta frames into compact summaries (saving >95% tokens).",
            "inputSchema": {
                "type": "object",
                "required": ["session_id"],
                "properties": {
                    "session_id": { "type": "string" },
                    "fold_deltas": { "type": "boolean", "default": true },
                    "include_heartbeat": { "type": "boolean", "default": false },
                    "limit": { "type": "integer", "default": 200 }
                }
            }
        },
        {
            "name": "search_traffic",
            "description": "Full-text search (SQLite FTS5) across captured HTTP URLs/bodies and SSE event payloads.",
            "inputSchema": {
                "type": "object",
                "required": ["query"],
                "properties": {
                    "query": { "type": "string" },
                    "session_id": { "type": "string" },
                    "exclude_session_id": { "type": "string" },
                    "kind": { "type": "string", "enum": ["http", "sse"] },
                    "limit": { "type": "integer", "default": 30 }
                }
            }
        },
        {
            "name": "find_anomalies",
            "description": "Automatically scan captured traffic for OpenCode failure patterns: HTTP 4xx/5xx, upstream errors, slow requests, reconnect storms, orphan message.part.delta events (dropped by client), unanswered permission/question prompts, and directory mismatches.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "exclude_session_id": { "type": "string" },
                    "slow_http_ms": { "type": "integer", "default": 3000 },
                    "limit": { "type": "integer", "default": 50 }
                }
            }
        },
        {
            "name": "get_http_detail",
            "description": "Inspect full HTTP request/response headers and body for a trace ID, with safe pagination/truncation.",
            "inputSchema": {
                "type": "object",
                "required": ["trace_id"],
                "properties": {
                    "trace_id": { "type": "string" },
                    "max_body_chars": { "type": "integer", "default": 8000 },
                    "body_offset": { "type": "integer", "default": 0 }
                }
            }
        },
        {
            "name": "get_sse_range",
            "description": "Fetch SSE events in a specific sequence range (or filtered by session/event_type). Set fold_deltas=false to inspect raw frame-by-frame deltas.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "event_type": { "type": "string" },
                    "from_seq": { "type": "integer" },
                    "to_seq": { "type": "integer" },
                    "fold_deltas": { "type": "boolean", "default": true },
                    "limit": { "type": "integer", "default": 150 }
                }
            }
        },
        {
            "name": "reconstruct_state_at",
            "description": "Replay recorded SSE events through the Rust ShadowStore up to at_seq (inclusive) and return the exact client Session/Message/Part/Status/Todo/Permission/Question snapshot at that moment.",
            "inputSchema": {
                "type": "object",
                "required": ["session_id"],
                "properties": {
                    "session_id": { "type": "string" },
                    "at_seq": { "type": "integer", "description": "Optional inclusive SSE sequence number ceiling" }
                }
            }
        },
        {
            "name": "create_mock_rule",
            "description": "Create an HTTP mock, JSON response patch, SSE stream, or dynamic request/response breakpoint rule (with optional HTTP-triggered SSE sequence).",
            "inputSchema": {
                "type": "object",
                "required": ["name", "path_pattern"],
                "properties": {
                    "name": { "type": "string" },
                    "method": { "type": "string", "default": "GET" },
                    "path_pattern": { "type": "string", "description": "Path pattern supporting :param, *, and regex:<expr>" },
                    "query_pattern": { "type": "string" },
                    "body_contains": { "type": "string" },
                    "directory_filter": { "type": "string" },
                    "action_type": { "type": "string", "enum": ["direct", "patch", "breakpoint", "sse_stream"], "default": "direct" },
                    "breakpoint_phase": { "type": "string", "enum": ["request", "response", "both"], "default": "request" },
                    "one_shot": { "type": "boolean", "default": false },
                    "status": { "type": "integer", "default": 200 },
                    "delay_ms": { "type": "integer", "default": 0 },
                    "headers": { "type": "object" },
                    "body": { "description": "JSON response body for direct mocks" },
                    "patch": { "description": "JSON merge patch object for patch rules" },
                    "sse_sequence": { "type": "array", "description": "Optional timed SSE events triggered on HTTP match" }
                }
            }
        },
        {
            "name": "create_sse_mock_rule",
            "description": "Create a dynamic SSE stream interception rule to drop, JSON-patch, replace, delay, or breakpoint matching live OpenCode SSE events.",
            "inputSchema": {
                "type": "object",
                "required": ["name", "event_type_pattern"],
                "properties": {
                    "name": { "type": "string" },
                    "event_type_pattern": { "type": "string", "description": "Exact event type, wildcard (e.g. message.part.*), or regex:<expr>" },
                    "session_filter": { "type": "string" },
                    "directory_filter": { "type": "string" },
                    "payload_contains": { "type": "string" },
                    "action_type": { "type": "string", "enum": ["drop", "patch", "replace", "delay", "breakpoint"], "default": "patch" },
                    "delay_ms": { "type": "integer", "default": 0 },
                    "patch": { "description": "JSON merge patch applied to SSE payload" },
                    "replace_events": { "type": "array", "description": "Replacement SSE steps when action_type=replace" },
                    "one_shot": { "type": "boolean", "default": false }
                }
            }
        },
        {
            "name": "inject_sse_event",
            "description": "Inject a synthetic OpenCode SSE event directly into connected clients (e.g., permission.asked, question.asked, session.error, tui.session.select).",
            "inputSchema": {
                "type": "object",
                "required": ["payload"],
                "properties": {
                    "directory": { "type": "string" },
                    "payload": { "type": "object", "description": "OpenCode BusEvent payload with { type, properties }" },
                    "apply_to_shadow": { "type": "boolean", "default": false }
                }
            }
        },
        {
            "name": "control_replay",
            "description": "Control session tape playback on the live proxy: list_tapes, load, play, pause, stop, step, seek, or tui_select.",
            "inputSchema": {
                "type": "object",
                "required": ["action"],
                "properties": {
                    "action": { "type": "string", "enum": ["status", "list_tapes", "load", "play", "pause", "stop", "step", "seek", "tui_select"] },
                    "tape_id": { "type": "string" },
                    "target_directory": { "type": "string" },
                    "speed": { "type": "number" },
                    "cursor": { "type": "integer" },
                    "session_id": { "type": "string" }
                }
            }
        }
    ])
}

async fn call_tool(backend: &McpBackend, name: &str, args: Value) -> Result<Value> {
    let storage = backend.storage();
    match name {
        "list_sessions" => {
            let ex = args.get("exclude_session_id").and_then(Value::as_str);
            let inc = args
                .get("include_ignored")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            Ok(serde_json::to_value(storage.list_sessions(ex, inc)?)?)
        }
        "get_session_timeline" => {
            let sid = args
                .get("session_id")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("missing session_id"))?;
            let fold = args
                .get("fold_deltas")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            let hb = args
                .get("include_heartbeat")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let limit = args
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(200) as usize;
            Ok(serde_json::to_value(
                storage.get_session_timeline(sid, fold, hb, limit)?,
            )?)
        }
        "search_traffic" => {
            let query = args
                .get("query")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("missing query"))?;
            let sid = args.get("session_id").and_then(Value::as_str);
            let ex = args.get("exclude_session_id").and_then(Value::as_str);
            let kind = args.get("kind").and_then(Value::as_str);
            let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(30) as usize;
            Ok(serde_json::to_value(
                storage.search_traffic(query, sid, kind, ex, limit)?,
            )?)
        }
        "find_anomalies" => {
            let opts = AnomalyOptions {
                session_id: args
                    .get("session_id")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                exclude_session_id: args
                    .get("exclude_session_id")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                slow_http_ms: args
                    .get("slow_http_ms")
                    .and_then(Value::as_i64)
                    .unwrap_or(3000),
                limit: args.get("limit").and_then(Value::as_u64).unwrap_or(50) as usize,
            };
            Ok(serde_json::to_value(find_anomalies(storage, &opts)?)?)
        }
        "get_http_detail" => {
            let tid = args
                .get("trace_id")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("missing trace_id"))?;
            let max_chars = args
                .get("max_body_chars")
                .and_then(Value::as_u64)
                .unwrap_or(8000) as usize;
            let offset = args
                .get("body_offset")
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize;
            let mut trace = storage
                .get_http_trace(tid)?
                .ok_or_else(|| anyhow::anyhow!("trace '{tid}' not found"))?;
            let req_truncated = slice_body(&mut trace.req_body, offset, max_chars);
            let res_truncated = slice_body(&mut trace.res_body, offset, max_chars);
            Ok(json!({
                "trace": trace,
                "pagination": {
                    "offset": offset,
                    "max_body_chars": max_chars,
                    "req_body_truncated": req_truncated,
                    "res_body_truncated": res_truncated
                }
            }))
        }
        "get_sse_range" => {
            let sid = args.get("session_id").and_then(Value::as_str);
            let et = args.get("event_type").and_then(Value::as_str);
            let from_seq = args.get("from_seq").and_then(Value::as_i64);
            let to_seq = args.get("to_seq").and_then(Value::as_i64);
            let fold = args
                .get("fold_deltas")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(150) as usize;
            let events = storage.list_sse_events(
                sid, et, None, from_seq, to_seq, true, None, limit, false,
            )?;
            if fold {
                Ok(serde_json::to_value(fold_sse_events(events))?)
            } else {
                Ok(serde_json::to_value(events)?)
            }
        }
        "reconstruct_state_at" => {
            let sid = args
                .get("session_id")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("missing session_id"))?;
            let at_seq = args.get("at_seq").and_then(Value::as_i64);
            let store = storage.reconstruct_session_state(sid, at_seq)?;
            Ok(store.snapshot_json(Some(sid)))
        }
        "create_mock_rule" => {
            let name = args
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("missing name"))?
                .to_string();
            let path_pattern = args
                .get("path_pattern")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("missing path_pattern"))?
                .to_string();
            let headers: BTreeMap<String, String> = args
                .get("headers")
                .cloned()
                .and_then(|v| serde_json::from_value(v).ok())
                .unwrap_or_default();
            let sse_sequence: Vec<MockSseStep> = args
                .get("sse_sequence")
                .cloned()
                .and_then(|v| serde_json::from_value(v).ok())
                .unwrap_or_default();
            let rule = MockRule {
                id: format!("rule_{}", &uuid::Uuid::new_v4().simple().to_string()[..10]),
                name,
                enabled: true,
                priority: 50,
                method: args
                    .get("method")
                    .and_then(Value::as_str)
                    .unwrap_or("GET")
                    .to_uppercase(),
                path_pattern,
                query_pattern: args
                    .get("query_pattern")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                body_contains: args
                    .get("body_contains")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                directory_filter: args
                    .get("directory_filter")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                action_type: MockActionType::parse(
                    args.get("action_type")
                        .and_then(Value::as_str)
                        .unwrap_or("direct"),
                ),
                breakpoint_phase: BreakpointPhase::parse(
                    args.get("breakpoint_phase")
                        .and_then(Value::as_str)
                        .unwrap_or("request"),
                ),
                one_shot: args.get("one_shot").and_then(Value::as_bool).unwrap_or(false),
                hit_count: 0,
                status: args.get("status").and_then(Value::as_u64).unwrap_or(200) as u16,
                delay_ms: args.get("delay_ms").and_then(Value::as_u64).unwrap_or(0),
                headers,
                body: args.get("body").cloned(),
                patch: args.get("patch").cloned(),
                sse_sequence,
                created_at: now_ms(),
            };
            storage.upsert_mock_rule(&rule)?;
            if let McpBackend::Live(state) = backend {
                state.ensure_intercept_if_passthrough();
                state.broadcast(ControlWsEvent::RulesUpdated);
            }
            Ok(serde_json::to_value(rule)?)
        }
        "create_sse_mock_rule" => {
            let name = args
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("missing name"))?
                .to_string();
            let event_type_pattern = args
                .get("event_type_pattern")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("missing event_type_pattern"))?
                .to_string();
            let replace_events: Vec<MockSseStep> = args
                .get("replace_events")
                .cloned()
                .and_then(|v| serde_json::from_value(v).ok())
                .unwrap_or_default();
            let rule = SseMockRule {
                id: format!("sserule_{}", &uuid::Uuid::new_v4().simple().to_string()[..10]),
                name,
                enabled: true,
                priority: 50,
                event_type_pattern,
                session_filter: args
                    .get("session_filter")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                directory_filter: args
                    .get("directory_filter")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                payload_contains: args
                    .get("payload_contains")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                action_type: SseMockActionType::parse(
                    args.get("action_type")
                        .and_then(Value::as_str)
                        .unwrap_or("patch"),
                ),
                delay_ms: args.get("delay_ms").and_then(Value::as_u64).unwrap_or(0),
                patch: args.get("patch").cloned(),
                replace_events,
                one_shot: args.get("one_shot").and_then(Value::as_bool).unwrap_or(false),
                hit_count: 0,
                created_at: now_ms(),
            };
            storage.upsert_sse_mock_rule(&rule)?;
            if let McpBackend::Live(state) = backend {
                state.ensure_intercept_if_passthrough();
                state.broadcast(ControlWsEvent::SseRulesUpdated);
            }
            Ok(serde_json::to_value(rule)?)
        }
        "inject_sse_event" => {
            let payload = args
                .get("payload")
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("missing payload"))?;
            let dir = args
                .get("directory")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);
            let apply_shadow = args
                .get("apply_to_shadow")
                .and_then(Value::as_bool)
                .unwrap_or(false);

            match backend {
                McpBackend::Live(state) => {
                    let target_dir = dir.unwrap_or_else(|| state.primary_active_directory());
                    if apply_shadow {
                        state.shadow.write().unwrap().apply_event(&payload);
                    }
                    let rec = state.emit_sse(SseSource::Injected, &target_dir, payload);
                    Ok(json!({ "ok": true, "recorded": rec }))
                }
                McpBackend::Standalone {
                    control_url: Some(url),
                    http,
                    ..
                } => {
                    let res = http
                        .post(format!("{}/api/sse/inject", url.trim_end_matches('/')))
                        .json(&json!({
                            "directory": dir,
                            "payload": payload,
                            "apply_to_shadow": apply_shadow
                        }))
                        .send()
                        .await?
                        .json::<Value>()
                        .await?;
                    Ok(res)
                }
                McpBackend::Standalone {
                    control_url: None, ..
                } => Err(anyhow::anyhow!(
                    "inject_sse_event requires live server or --control-url"
                )),
            }
        }
        "control_replay" => {
            let action = args
                .get("action")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("missing action"))?;
            if action == "list_tapes" {
                return Ok(serde_json::to_value(storage.list_tapes()?)?);
            }
            match backend {
                McpBackend::Live(state) => match action {
                    "status" => Ok(serde_json::to_value(state.replay.lock().unwrap().status.clone())?),
                    "load" => {
                        let tid = args
                            .get("tape_id")
                            .and_then(Value::as_str)
                            .ok_or_else(|| anyhow::anyhow!("missing tape_id"))?;
                        let st = replay::load_tape(
                            state,
                            tid,
                            args.get("target_directory")
                                .and_then(Value::as_str)
                                .map(ToOwned::to_owned),
                            true,
                            args.get("speed").and_then(Value::as_f64),
                            None,
                        )?;
                        Ok(serde_json::to_value(st)?)
                    }
                    "play" => Ok(serde_json::to_value(replay::play(state)?)?),
                    "pause" => Ok(serde_json::to_value(replay::pause(state))?),
                    "stop" => Ok(serde_json::to_value(replay::stop(state))?),
                    "step" => Ok(serde_json::to_value(replay::step_next(state)?)?),
                    "seek" => {
                        let c = args
                            .get("cursor")
                            .and_then(Value::as_u64)
                            .ok_or_else(|| anyhow::anyhow!("missing cursor"))?
                            as usize;
                        Ok(serde_json::to_value(replay::seek(state, c, true)?)?)
                    }
                    "tui_select" => {
                        let sid = args
                            .get("session_id")
                            .and_then(Value::as_str)
                            .map(ToOwned::to_owned);
                        Ok(replay::select_in_tui(state, sid)?)
                    }
                    other => Err(anyhow::anyhow!("unknown replay action: {other}")),
                },
                McpBackend::Standalone {
                    control_url: Some(url),
                    http,
                    ..
                } => {
                    let base = url.trim_end_matches('/');
                    let res = match action {
                        "status" => http.get(format!("{base}/api/replay/status")).send().await?,
                        "load" => http.post(format!("{base}/api/replay/load")).json(&args).send().await?,
                        "play" => http.post(format!("{base}/api/replay/play")).send().await?,
                        "pause" => http.post(format!("{base}/api/replay/pause")).send().await?,
                        "stop" => http.post(format!("{base}/api/replay/stop")).send().await?,
                        "step" => http.post(format!("{base}/api/replay/step")).send().await?,
                        "seek" => http.post(format!("{base}/api/replay/seek")).json(&args).send().await?,
                        "tui_select" => http.post(format!("{base}/api/replay/tui-select")).json(&args).send().await?,
                        other => return Err(anyhow::anyhow!("unknown replay action: {other}")),
                    };
                    Ok(res.json::<Value>().await?)
                }
                McpBackend::Standalone {
                    control_url: None, ..
                } => Err(anyhow::anyhow!(
                    "control_replay requires live server or --control-url"
                )),
            }
        }
        _ => Err(anyhow::anyhow!("unknown tool: {name}")),
    }
}

fn slice_body(body: &mut Option<String>, offset: usize, max_chars: usize) -> bool {
    let Some(s) = body.as_ref() else {
        return false;
    };
    let total = s.chars().count();
    if offset == 0 && total <= max_chars {
        return false;
    }
    let sliced: String = s.chars().skip(offset).take(max_chars).collect();
    let truncated = offset + max_chars < total;
    *body = Some(sliced);
    truncated
}
