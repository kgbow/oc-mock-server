use crate::sse::{handle_directory_event, handle_global_event};
use crate::state::{AppState, PendingBreakpointEntry};
use axum::body::Body;
use axum::extract::ws::{Message as AxumWsMsg, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use futures_util::{SinkExt, StreamExt};
use oc_core::*;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::oneshot;
use tokio_tungstenite::tungstenite::Message as TungMsg;
use tower_http::cors::{Any, CorsLayer};

const MAX_CAPTURE_BODY_BYTES: usize = 2 * 1024 * 1024;

pub fn build_proxy_router(state: Arc<AppState>) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any)
        .expose_headers([
            HeaderName::from_static("link"),
            HeaderName::from_static("x-next-cursor"),
            HeaderName::from_static("x-oc-proxy-mocked"),
            HeaderName::from_static("x-oc-proxy-trace-id"),
        ]);

    Router::new()
        .route("/global/event", get(handle_global_event))
        .route("/event", get(handle_directory_event))
        .route("/pty/{pty_id}/connect", get(handle_pty_ws))
        .fallback(handle_http_proxy)
        .layer(cors)
        .with_state(state)
}

async fn handle_pty_ws(
    State(state): State<Arc<AppState>>,
    Path(pty_id): Path<String>,
    req_headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let Some(upstream) = state.config.upstream_url.clone() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "PTY WebSocket unavailable in offline mode",
        )
            .into_response();
    };
    let ws_base = upstream
        .trim_end_matches('/')
        .replacen("https://", "wss://", 1)
        .replacen("http://", "ws://", 1);
    let target = format!("{ws_base}/pty/{pty_id}/connect");
    let auth = req_headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(ToOwned::to_owned);

    ws.on_upgrade(move |socket| bridge_pty_ws(socket, target, auth))
}

async fn bridge_pty_ws(client_ws: WebSocket, target_url: String, _auth: Option<String>) {
    let Ok((upstream_ws, _)) = tokio_tungstenite::connect_async(&target_url).await else {
        tracing::warn!("failed to connect upstream PTY WebSocket: {target_url}");
        return;
    };

    let (mut c_sink, mut c_stream) = client_ws.split();
    let (mut u_sink, mut u_stream) = upstream_ws.split();

    let c_to_u = async {
        while let Some(Ok(msg)) = c_stream.next().await {
            let mapped = match msg {
                AxumWsMsg::Text(t) => TungMsg::Text(t.to_string().into()),
                AxumWsMsg::Binary(b) => TungMsg::Binary(b),
                AxumWsMsg::Ping(p) => TungMsg::Ping(p),
                AxumWsMsg::Pong(p) => TungMsg::Pong(p),
                AxumWsMsg::Close(_) => {
                    let _ = u_sink.close().await;
                    break;
                }
            };
            if u_sink.send(mapped).await.is_err() {
                break;
            }
        }
    };

    let u_to_c = async {
        while let Some(Ok(msg)) = u_stream.next().await {
            let mapped = match msg {
                TungMsg::Text(t) => AxumWsMsg::Text(t.to_string().into()),
                TungMsg::Binary(b) => AxumWsMsg::Binary(b),
                TungMsg::Ping(p) => AxumWsMsg::Ping(p),
                TungMsg::Pong(p) => AxumWsMsg::Pong(p),
                TungMsg::Close(_) => {
                    let _ = c_sink.close().await;
                    break;
                }
                TungMsg::Frame(_) => continue,
            };
            if c_sink.send(mapped).await.is_err() {
                break;
            }
        }
    };

    tokio::select! {
        _ = c_to_u => {},
        _ = u_to_c => {},
    }
}

async fn handle_http_proxy(State(state): State<Arc<AppState>>, req: Request) -> Response {
    let started_at = now_ms();
    let trace_id = format!("tr_{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
    let method = req.method().clone();
    let uri = req.uri().clone();
    let path = uri.path().to_string();
    let query = uri.query().map(ToOwned::to_owned);
    let req_headers_map = headers_to_map(req.headers());
    let directory = extract_directory(&query, req.headers());
    if let Some(ref d) = directory {
        state.record_directory(d);
    }

    let mut session_id = extract_session_from_path(&path);

    let body_bytes = match axum::body::to_bytes(req.into_body(), MAX_CAPTURE_BODY_BYTES).await {
        Ok(b) => b,
        Err(err) => {
            return (
                StatusCode::PAYLOAD_TOO_LARGE,
                format!("request body exceeded capture limit: {err}"),
            )
                .into_response();
        }
    };

    let req_size = body_bytes.len();
    let mut req_body_str = if body_bytes.is_empty() {
        None
    } else {
        Some(String::from_utf8_lossy(&body_bytes).into_owned())
    };
    let mut eff_req_headers = req_headers_map.clone();

    let mut trace = HttpTrace {
        id: trace_id.clone(),
        started_at,
        completed_at: None,
        duration_ms: None,
        method: method.to_string(),
        path: path.clone(),
        query: query.clone(),
        directory: directory.clone(),
        session_id: session_id.clone(),
        status: None,
        req_headers: req_headers_map,
        req_body: req_body_str.clone(),
        req_size,
        res_headers: BTreeMap::new(),
        res_body: None,
        res_size: 0,
        mocked: false,
        mock_rule_id: None,
        upstream_error: None,
        pending: true,
    };

    if state.is_recording() {
        let _ = state.storage.insert_http_started(&trace);
        state.broadcast(ControlWsEvent::HttpStarted {
            trace: trace_to_summary(&trace),
        });
    }

    // 1. Evaluate Mock / Patch / Breakpoint Rules
    let matched_rule = state
        .storage
        .find_matching_rule(method.as_str(), &path, directory.as_deref())
        .ok()
        .flatten();

    let mut patch_to_apply: Option<(String, Value)> = None;

    if let Some(rule) = matched_rule {
        match rule.action_type {
            MockActionType::Direct => {
                if rule.delay_ms > 0 {
                    tokio::time::sleep(Duration::from_millis(rule.delay_ms)).await;
                }
                let body_text = match &rule.body {
                    Some(Value::String(s)) => s.clone(),
                    Some(v) => serde_json::to_string(v).unwrap_or_default(),
                    None => "{}".to_string(),
                };
                let mut res_headers = rule.headers.clone();
                res_headers
                    .entry("content-type".to_string())
                    .or_insert_with(|| "application/json".to_string());
                res_headers.insert("x-oc-proxy-mocked".into(), "true".into());
                return finalize_and_respond(
                    &state,
                    &mut trace,
                    rule.status,
                    res_headers,
                    body_text.into_bytes(),
                    true,
                    Some(rule.id),
                    None,
                );
            }
            MockActionType::Breakpoint => {
                let bp_id = format!("bp_{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
                let (tx, rx) = oneshot::channel();
                let bp = PendingBreakpoint {
                    id: bp_id.clone(),
                    trace_id: trace_id.clone(),
                    rule_id: rule.id.clone(),
                    rule_name: rule.name.clone(),
                    started_at,
                    method: method.to_string(),
                    path: path.clone(),
                    query: query.clone(),
                    directory: directory.clone(),
                    req_headers: eff_req_headers.clone(),
                    req_body: req_body_str.clone(),
                };
                state.breakpoints.insert(
                    bp_id.clone(),
                    PendingBreakpointEntry {
                        info: bp.clone(),
                        tx,
                    },
                );
                state.broadcast(ControlWsEvent::BreakpointAdded { breakpoint: bp });
                state.broadcast_status();

                let decision = tokio::time::timeout(Duration::from_secs(300), rx).await;
                state.breakpoints.remove(&bp_id);
                state.broadcast(ControlWsEvent::BreakpointResolved { id: bp_id });
                state.broadcast_status();

                match decision {
                    Ok(Ok(BreakpointDecision::Respond {
                        status,
                        mut headers,
                        body,
                    })) => {
                        headers
                            .entry("content-type".to_string())
                            .or_insert_with(|| "application/json".to_string());
                        headers.insert("x-oc-proxy-mocked".into(), "breakpoint".into());
                        return finalize_and_respond(
                            &state,
                            &mut trace,
                            status,
                            headers,
                            body.into_bytes(),
                            true,
                            Some(rule.id),
                            None,
                        );
                    }
                    Ok(Ok(BreakpointDecision::Forward {
                        req_body,
                        req_headers,
                    })) => {
                        if let Some(b) = req_body {
                            req_body_str = Some(b);
                        }
                        if let Some(h) = req_headers {
                            eff_req_headers = h;
                        }
                    }
                    Ok(Ok(BreakpointDecision::Abort { message })) => {
                        let msg = message.unwrap_or_else(|| "aborted at breakpoint".to_string());
                        return finalize_and_respond(
                            &state,
                            &mut trace,
                            502,
                            json_headers(),
                            serde_json::to_vec(&json!({ "error": msg })).unwrap_or_default(),
                            true,
                            Some(rule.id),
                            Some(msg),
                        );
                    }
                    _ => {
                        // Timed out or channel closed -> proceed with normal forward
                    }
                }
            }
            MockActionType::Patch => {
                if rule.delay_ms > 0 {
                    tokio::time::sleep(Duration::from_millis(rule.delay_ms)).await;
                }
                if let Some(p) = rule.patch {
                    patch_to_apply = Some((rule.id, p));
                }
            }
        }
    }

    // 2. Shadow Store Virtual Intercept (for Replay sessions or Offline mode)
    let is_offline = state.config.upstream_url.is_none();
    let is_replay_mode = state.mode() == ProxyMode::Replay;
    let mount_shadow = state.replay.lock().unwrap().status.mount_shadow;

    if method == Method::GET && (is_offline || is_replay_mode || mount_shadow) {
        if let Some(virtual_json) =
            try_serve_shadow_exact(&state, &path, is_offline || is_replay_mode)
        {
            let mut headers = json_headers();
            headers.insert("x-oc-proxy-mocked".into(), "shadow".into());
            return finalize_and_respond(
                &state,
                &mut trace,
                200,
                headers,
                serde_json::to_vec(&virtual_json).unwrap_or_default(),
                true,
                Some("shadow_store".into()),
                None,
            );
        }
    }

    // 3. Offline fallback when no upstream is configured
    let Some(upstream) = state.config.upstream_url.clone() else {
        if let Some((status, val)) =
            serve_offline_fallback(&state, &method, &path, directory.as_deref(), req_body_str.as_deref())
        {
            let mut headers = json_headers();
            headers.insert("x-oc-proxy-mocked".into(), "offline".into());
            return finalize_and_respond(
                &state,
                &mut trace,
                status,
                headers,
                serde_json::to_vec(&val).unwrap_or_default(),
                true,
                Some("offline_server".into()),
                None,
            );
        }
        return finalize_and_respond(
            &state,
            &mut trace,
            404,
            json_headers(),
            serde_json::to_vec(&json!({ "name": "NotFoundError", "data": { "message": format!("No upstream configured and no offline handler for {} {}", method, path) } })).unwrap_or_default(),
            true,
            Some("offline_404".into()),
            None,
        );
    };

    // 4. Forward to Upstream OpenCode Server
    let target_url = match &query {
        Some(q) if !q.is_empty() => format!("{}{}?{}", upstream.trim_end_matches('/'), path, q),
        _ => format!("{}{}", upstream.trim_end_matches('/'), path),
    };

    let req_method = reqwest::Method::from_bytes(method.as_str().as_bytes())
        .unwrap_or(reqwest::Method::GET);
    let mut builder = state.http_client.request(req_method, &target_url);

    for (k, v) in &eff_req_headers {
        let lower = k.to_lowercase();
        if matches!(
            lower.as_str(),
            "host" | "connection" | "keep-alive" | "transfer-encoding" | "content-length" | "accept-encoding"
        ) {
            continue;
        }
        builder = builder.header(k.as_str(), v.as_str());
    }

    if let Some(body) = req_body_str {
        builder = builder.body(body);
    }

    match builder.send().await {
        Ok(res) => {
            let status = res.status().as_u16();
            let mut res_headers = BTreeMap::new();
            for (k, v) in res.headers() {
                let name = k.as_str().to_lowercase();
                if matches!(
                    name.as_str(),
                    "connection" | "keep-alive" | "transfer-encoding" | "content-length"
                ) {
                    continue;
                }
                if let Ok(val) = v.to_str() {
                    res_headers.insert(name, val.to_string());
                }
            }

            let raw_bytes = match res.bytes().await {
                Ok(b) => b.to_vec(),
                Err(err) => {
                    let msg = format!("upstream read error: {err}");
                    return finalize_and_respond(
                        &state,
                        &mut trace,
                        502,
                        json_headers(),
                        serde_json::to_vec(&json!({ "error": msg })).unwrap_or_default(),
                        false,
                        None,
                        Some(msg),
                    );
                }
            };

            let mut final_bytes = raw_bytes;
            let mut mocked = false;
            let mut rule_id = None;

            // Hybrid Shadow Store merge on GET /session and GET /session/status
            if method == Method::GET && mount_shadow && status == 200 {
                if let Ok(mut parsed) = serde_json::from_slice::<Value>(&final_bytes) {
                    if merge_shadow_into_upstream(&state, &path, &mut parsed) {
                        if let Ok(encoded) = serde_json::to_vec(&parsed) {
                            final_bytes = encoded;
                        }
                    }
                }
            }

            // Apply Response JSON Patch if matched a Patch rule
            if let Some((rid, patch_val)) = patch_to_apply {
                if let Ok(mut parsed) = serde_json::from_slice::<Value>(&final_bytes) {
                    json_merge_patch(&mut parsed, &patch_val);
                    if let Ok(encoded) = serde_json::to_vec(&parsed) {
                        final_bytes = encoded;
                        mocked = true;
                        rule_id = Some(rid);
                        res_headers.insert("x-oc-proxy-mocked".into(), "patched".into());
                    }
                }
            }

            if session_id.is_none() {
                if let Ok(text) = std::str::from_utf8(&final_bytes) {
                    session_id = extract_session_from_body(&path, text);
                    trace.session_id = session_id;
                }
            }

            finalize_and_respond(
                &state,
                &mut trace,
                status,
                res_headers,
                final_bytes,
                mocked,
                rule_id,
                None,
            )
        }
        Err(err) => {
            let msg = format!("upstream connect error: {err}");
            finalize_and_respond(
                &state,
                &mut trace,
                502,
                json_headers(),
                serde_json::to_vec(&json!({ "error": msg })).unwrap_or_default(),
                false,
                None,
                Some(msg),
            )
        }
    }
}

fn finalize_and_respond(
    state: &Arc<AppState>,
    trace: &mut HttpTrace,
    status: u16,
    res_headers: BTreeMap<String, String>,
    body_bytes: Vec<u8>,
    mocked: bool,
    mock_rule_id: Option<String>,
    upstream_error: Option<String>,
) -> Response {
    let completed = now_ms();
    trace.completed_at = Some(completed);
    trace.duration_ms = Some((completed - trace.started_at).max(0));
    trace.status = Some(status);
    trace.res_headers = res_headers.clone();
    trace.res_size = body_bytes.len();
    trace.res_body = if body_bytes.is_empty() {
        None
    } else {
        let cap = body_bytes.len().min(MAX_CAPTURE_BODY_BYTES);
        Some(String::from_utf8_lossy(&body_bytes[..cap]).into_owned())
    };
    trace.mocked = mocked;
    trace.mock_rule_id = mock_rule_id;
    trace.upstream_error = upstream_error;
    trace.pending = false;

    if state.is_recording() {
        let _ = state.storage.complete_http_trace(trace);
        state.broadcast(ControlWsEvent::HttpCompleted {
            trace: trace_to_summary(trace),
        });
    }

    let mut builder = Response::builder().status(status);
    for (k, v) in res_headers {
        if let (Ok(hn), Ok(hv)) = (
            HeaderName::from_bytes(k.as_bytes()),
            HeaderValue::from_str(&v),
        ) {
            builder = builder.header(hn, hv);
        }
    }
    if let Ok(hv) = HeaderValue::from_str(&trace.id) {
        builder = builder.header("x-oc-proxy-trace-id", hv);
    }

    builder
        .body(Body::from(body_bytes))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

fn try_serve_shadow_exact(state: &AppState, path: &str, full_takeover: bool) -> Option<Value> {
    let shadow = state.shadow.read().unwrap();
    let trimmed = path.trim_end_matches('/');

    if trimmed == "/session" && full_takeover {
        return Some(Value::Array(shadow.sessions_list()));
    }
    if trimmed == "/session/status" && full_takeover {
        return Some(shadow.status_map());
    }
    if trimmed == "/permission" && full_takeover {
        return Some(Value::Array(shadow.all_permissions()));
    }
    if trimmed == "/question" && full_takeover {
        return Some(Value::Array(shadow.all_questions()));
    }

    let segs: Vec<&str> = trimmed.trim_start_matches('/').split('/').collect();
    if segs.len() >= 2 && segs[0] == "session" {
        let sid = segs[1];
        if shadow.session_by_id(sid).is_some() {
            return match segs.as_slice() {
                ["session", _] => shadow.session_by_id(sid),
                ["session", _, "message"] => Some(Value::Array(shadow.messages_with_parts(sid))),
                ["session", _, "todo"] => Some(Value::Array(shadow.todos_for(sid))),
                ["session", _, "diff"] => Some(Value::Array(shadow.diffs_for(sid))),
                ["session", _, "children"] => Some(json!([])),
                _ => None,
            };
        }
    }
    None
}

fn merge_shadow_into_upstream(state: &AppState, path: &str, upstream_val: &mut Value) -> bool {
    let shadow = state.shadow.read().unwrap();
    if shadow.sessions.is_empty() {
        return false;
    }
    let trimmed = path.trim_end_matches('/');
    if trimmed == "/session" {
        if let Some(arr) = upstream_val.as_array_mut() {
            let mut changed = false;
            for sess in shadow.sessions_list() {
                let sid = sess.get("id").and_then(Value::as_str);
                let exists = arr.iter().any(|item| item.get("id").and_then(Value::as_str) == sid);
                if !exists {
                    arr.insert(0, sess);
                    changed = true;
                }
            }
            return changed;
        }
    } else if trimmed == "/session/status" {
        if let Some(map) = upstream_val.as_object_mut() {
            let mut changed = false;
            for (k, v) in &shadow.statuses {
                map.insert(k.clone(), v.clone());
                changed = true;
            }
            return changed;
        }
    } else if trimmed == "/permission" {
        if let Some(arr) = upstream_val.as_array_mut() {
            let perms = shadow.all_permissions();
            if !perms.is_empty() {
                arr.extend(perms);
                return true;
            }
        }
    } else if trimmed == "/question" {
        if let Some(arr) = upstream_val.as_array_mut() {
            let qs = shadow.all_questions();
            if !qs.is_empty() {
                arr.extend(qs);
                return true;
            }
        }
    }
    false
}

/// Serve full OpenCode bootstrap endpoints in offline mode so an OpenCode Web or TUI client
/// can connect and inspect/replay sessions without running a real OpenCode server.
fn serve_offline_fallback(
    state: &AppState,
    method: &Method,
    path: &str,
    directory: Option<&str>,
    _req_body: Option<&str>,
) -> Option<(u16, Value)> {
    let dir = directory
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| state.primary_active_directory());
    let trimmed = path.trim_end_matches('/');
    let shadow = state.shadow.read().unwrap();

    if *method == Method::GET {
        return match trimmed {
            "/global/health" => Some((200, json!({ "healthy": true, "version": "0.1.0-mock" }))),
            "/doc" => Some((200, state.openapi.read().unwrap().doc.clone())),
            "/path" => Some((
                200,
                json!({
                    "home": std::env::var("HOME").unwrap_or_else(|_| "/tmp".into()),
                    "state": "/tmp/oc-mock/state",
                    "config": "/tmp/oc-mock/config",
                    "worktree": dir,
                    "directory": dir
                }),
            )),
            "/global/config" | "/config" => Some((200, json!({}))),
            "/project" => Some((200, Value::Array(shadow.projects_list()))),
            "/project/current" => Some((
                200,
                shadow
                    .projects_list()
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| json!({ "id": "proj_offline", "worktree": dir, "time": { "created": now_ms(), "updated": now_ms() } })),
            )),
            "/provider" => Some((200, json!({ "all": [], "connected": [], "default": {} }))),
            "/provider/auth" => Some((200, json!({}))),
            "/agent" => Some((200, json!([{ "name": "build", "description": "Mock build agent", "mode": "primary" }]))),
            "/skill" | "/command" | "/lsp" | "/formatter" => Some((200, json!([]))),
            "/mcp" => Some((200, json!({}))),
            "/vcs" => Some((
                200,
                json!({ "branch": shadow.vcs_branch.clone().unwrap_or_else(|| "main".into()) }),
            )),
            _ => None,
        };
    }

    if *method == Method::POST && trimmed == "/log" {
        return Some((200, json!(true)));
    }
    None
}

fn json_merge_patch(target: &mut Value, patch: &Value) {
    if let Value::Object(patch_map) = patch {
        if !target.is_object() {
            *target = Value::Object(serde_json::Map::new());
        }
        let target_map = target.as_object_mut().unwrap();
        for (k, v) in patch_map {
            if v.is_null() {
                target_map.remove(k);
            } else {
                let entry = target_map.entry(k.clone()).or_insert(Value::Null);
                json_merge_patch(entry, v);
            }
        }
    } else {
        *target = patch.clone();
    }
}

fn headers_to_map(headers: &HeaderMap) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for (k, v) in headers {
        if let Ok(val) = v.to_str() {
            map.insert(k.as_str().to_lowercase(), val.to_string());
        }
    }
    map
}

fn extract_directory(query: &Option<String>, headers: &HeaderMap) -> Option<String> {
    if let Some(q) = query {
        for pair in q.split('&') {
            if let Some((k, v)) = pair.split_once('=') {
                if k == "directory" && !v.is_empty() {
                    return Some(percent_decode(v));
                }
            }
        }
    }
    headers
        .get("x-opencode-directory")
        .and_then(|v| v.to_str().ok())
        .filter(|s| !s.is_empty())
        .map(percent_decode)
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(hex) = std::str::from_utf8(&bytes[i + 1..i + 3]) {
                if let Ok(val) = u8::from_str_radix(hex, 16) {
                    out.push(val);
                    i += 3;
                    continue;
                }
            }
        }
        if bytes[i] == b'+' {
            out.push(b' ');
        } else {
            out.push(bytes[i]);
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn json_headers() -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    m.insert("content-type".into(), "application/json".into());
    m
}

fn trace_to_summary(t: &HttpTrace) -> HttpTraceSummary {
    HttpTraceSummary {
        id: t.id.clone(),
        started_at: t.started_at,
        duration_ms: t.duration_ms,
        method: t.method.clone(),
        path: t.path.clone(),
        query: t.query.clone(),
        directory: t.directory.clone(),
        session_id: t.session_id.clone(),
        status: t.status,
        req_size: t.req_size,
        res_size: t.res_size,
        mocked: t.mocked,
        mock_rule_id: t.mock_rule_id.clone(),
        upstream_error: t.upstream_error.clone(),
        pending: t.pending,
    }
}
