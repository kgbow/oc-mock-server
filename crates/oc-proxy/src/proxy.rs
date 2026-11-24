use crate::sse::{handle_directory_event, handle_global_event};
use crate::state::{AppState, PendingBreakpointEntry};
use axum::body::Body;
use axum::extract::ws::{Message as AxumWsMsg, WebSocket, WebSocketUpgrade};
use axum::extract::{DefaultBodyLimit, Path, Request, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use bytes::{Bytes, BytesMut};
use futures_util::{FutureExt, SinkExt, StreamExt};
use oc_core::*;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::oneshot;
use tokio_tungstenite::tungstenite::Message as TungMsg;
use tower_http::cors::{Any, CorsLayer};

const SYNC_BUFFER_SWITCH_BYTES: usize = 64 * 1024;

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
        .layer(DefaultBodyLimit::disable())
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

#[derive(Default)]
struct BodyCapture {
    total_bytes: usize,
    captured: Vec<u8>,
    capture_enabled: bool,
}

impl BodyCapture {
    fn new(capture_enabled: bool) -> Self {
        Self {
            total_bytes: 0,
            captured: Vec::new(),
            capture_enabled,
        }
    }

    fn observe_chunk(&mut self, chunk: &[u8]) {
        self.total_bytes = self.total_bytes.saturating_add(chunk.len());
        if self.capture_enabled {
            self.captured.extend_from_slice(chunk);
        }
    }

    fn snapshot_size_and_body(&self) -> (usize, Option<String>) {
        let body = if !self.capture_enabled || self.captured.is_empty() {
            None
        } else {
            Some(String::from_utf8_lossy(&self.captured).into_owned())
        };
        (self.total_bytes, body)
    }

    fn into_size_and_body(self) -> (usize, Option<String>) {
        let body = if !self.capture_enabled || self.captured.is_empty() {
            None
        } else {
            Some(String::from_utf8_lossy(&self.captured).into_owned())
        };
        (self.total_bytes, body)
    }
}

fn full_utf8_capture(bytes: &[u8]) -> Option<String> {
    if bytes.is_empty() {
        None
    } else {
        Some(String::from_utf8_lossy(bytes).into_owned())
    }
}

/// Drain a request body stream while counting its total size and capturing its full content
/// when `capture` is enabled, without rejecting large payloads.
async fn drain_body_for_trace(body: Body, capture: bool) -> Result<(usize, Option<String>), String> {
    let mut cap = BodyCapture::new(capture);
    let mut stream = body.into_data_stream();
    while let Some(chunk_res) = stream.next().await {
        let chunk = chunk_res.map_err(|e| e.to_string())?;
        cap.observe_chunk(&chunk);
    }
    Ok(cap.into_size_and_body())
}

/// Collect full request bytes (used only when an active Breakpoint suspends the request for editing).
async fn collect_full_body(body: Body) -> Result<Bytes, String> {
    let mut buf = BytesMut::new();
    let mut stream = body.into_data_stream();
    while let Some(chunk_res) = stream.next().await {
        let chunk = chunk_res.map_err(|e| e.to_string())?;
        buf.extend_from_slice(&chunk);
    }
    Ok(buf.freeze())
}

enum OutgoingReqBody {
    Empty,
    Full(Bytes),
    Streaming(reqwest::Body),
}

fn into_sync_stream<S, T, E>(
    stream: S,
) -> impl futures_util::Stream<Item = Result<T, E>> + Send + Sync + 'static
where
    S: futures_util::Stream<Item = Result<T, E>> + Send + 'static,
    T: Send + 'static,
    E: Send + 'static,
{
    let mutex = Mutex::new(Box::pin(stream));
    futures_util::stream::poll_fn(move |cx| mutex.lock().unwrap().as_mut().poll_next(cx))
}

/// Stream the incoming request body to upstream while tee-capturing the full body when recording.
async fn prepare_streaming_req_body(
    req_body: Body,
    req_capture: Arc<Mutex<BodyCapture>>,
) -> Result<OutgoingReqBody, String> {
    let mut data_stream = req_body.into_data_stream();
    let first_chunk = loop {
        match data_stream.next().await {
            None => return Ok(OutgoingReqBody::Empty),
            Some(Ok(chunk)) if chunk.is_empty() => continue,
            Some(Ok(chunk)) => {
                req_capture.lock().unwrap().observe_chunk(&chunk);
                break chunk;
            }
            Some(Err(err)) => return Err(format!("client request body stream error: {err}")),
        }
    };

    let mut prefix = vec![first_chunk];
    let mut prefix_len = prefix[0].len();
    if prefix_len < SYNC_BUFFER_SWITCH_BYTES {
        loop {
            match data_stream.next().now_or_never() {
                Some(None) => {
                    if prefix.len() == 1 {
                        return Ok(OutgoingReqBody::Full(prefix.pop().unwrap()));
                    }
                    let mut combined = BytesMut::with_capacity(prefix_len);
                    for b in prefix {
                        combined.extend_from_slice(&b);
                    }
                    return Ok(OutgoingReqBody::Full(combined.freeze()));
                }
                Some(Some(Ok(chunk))) => {
                    if !chunk.is_empty() {
                        req_capture.lock().unwrap().observe_chunk(&chunk);
                        prefix_len += chunk.len();
                        prefix.push(chunk);
                        if prefix_len >= SYNC_BUFFER_SWITCH_BYTES {
                            break;
                        }
                    }
                }
                Some(Some(Err(err))) => {
                    return Err(format!("client request body stream error: {err}"));
                }
                None => break,
            }
        }
    }

    let prefix_stream =
        futures_util::stream::iter(prefix.into_iter().map(Ok::<Bytes, axum::Error>));
    let cap_tail = Arc::clone(&req_capture);
    let tail_stream = data_stream.map(move |res| {
        if let Ok(ref chunk) = res {
            cap_tail.lock().unwrap().observe_chunk(chunk);
        }
        res
    });
    let sync_stream = into_sync_stream(prefix_stream.chain(tail_stream));
    Ok(OutgoingReqBody::Streaming(reqwest::Body::wrap_stream(
        sync_stream,
    )))
}

struct StreamTraceFinalizer {
    state: Arc<AppState>,
    recording: bool,
    trace: HttpTrace,
    req_capture: Option<Arc<Mutex<BodyCapture>>>,
    res_capture: BodyCapture,
    extract_session_body: bool,
    finished: bool,
}

impl StreamTraceFinalizer {
    fn finish(&mut self, upstream_err: Option<String>) {
        if self.finished {
            return;
        }
        self.finished = true;

        if let Some(ref rc) = self.req_capture {
            let (req_sz, req_bd) = rc.lock().unwrap().snapshot_size_and_body();
            self.trace.req_size = req_sz;
            if self.recording {
                self.trace.req_body = req_bd;
            }
        }

        let completed = now_ms();
        self.trace.completed_at = Some(completed);
        self.trace.duration_ms = Some((completed - self.trace.started_at).max(0));
        self.trace.res_size = self.res_capture.total_bytes;

        if self.extract_session_body && self.trace.session_id.is_none() {
            if let Ok(text) = std::str::from_utf8(&self.res_capture.captured) {
                self.trace.session_id = extract_session_from_body(&self.trace.path, text);
            }
        }

        if self.recording {
            self.trace.res_body = if self.res_capture.captured.is_empty() {
                None
            } else {
                Some(String::from_utf8_lossy(&self.res_capture.captured).into_owned())
            };
        }
        if upstream_err.is_some() {
            self.trace.upstream_error = upstream_err;
        }
        self.trace.pending = false;

        if self.recording {
            let _ = self.state.storage.complete_http_trace(&self.trace);
            self.state.broadcast(ControlWsEvent::HttpCompleted {
                trace: trace_to_summary(&self.trace),
            });
        }
    }
}

impl Drop for StreamTraceFinalizer {
    fn drop(&mut self) {
        if !self.finished {
            self.finish(Some(
                "downstream client disconnected before response completed".into(),
            ));
        }
    }
}

async fn handle_http_proxy(State(state): State<Arc<AppState>>, req: Request) -> Response {
    let (parts, req_body) = req.into_parts();
    let started_at = now_ms();
    let trace_id = format!("tr_{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
    let method = parts.method;
    let uri = parts.uri;
    let path = uri.path().to_string();
    let query = uri.query().map(ToOwned::to_owned);
    let req_headers_map = headers_to_map(&parts.headers);
    let directory = extract_directory(&query, &parts.headers);
    if let Some(ref d) = directory {
        state.record_directory(d);
    }

    let mut session_id = extract_session_from_path(&path);
    let initial_req_size_hint = parts
        .headers
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(0);
    let mut eff_req_headers = req_headers_map.clone();
    let recording = state.is_recording();

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
        req_body: None,
        req_size: initial_req_size_hint,
        res_headers: BTreeMap::new(),
        res_body: None,
        res_size: 0,
        mocked: false,
        mock_rule_id: None,
        upstream_error: None,
        pending: true,
    };

    if recording {
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
    let mut breakpoint_forward_body: Option<Bytes> = None;
    let mut req_body_opt = Some(req_body);

    if let Some(rule) = matched_rule {
        match rule.action_type {
            MockActionType::Direct => {
                match drain_body_for_trace(req_body_opt.take().unwrap(), recording).await {
                    Ok((sz, bd)) => {
                        trace.req_size = sz;
                        trace.req_body = bd;
                    }
                    Err(err) => {
                        return finalize_and_respond(
                            &state,
                            recording,
                            &mut trace,
                            400,
                            json_headers(),
                            serde_json::to_vec(&json!({ "error": err })).unwrap_or_default(),
                            false,
                            None,
                            Some(err),
                        );
                    }
                }
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
                    recording,
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
                let raw_req_bytes = match collect_full_body(req_body_opt.take().unwrap()).await {
                    Ok(b) => b,
                    Err(err) => {
                        return finalize_and_respond(
                            &state,
                            recording,
                            &mut trace,
                            400,
                            json_headers(),
                            serde_json::to_vec(&json!({ "error": err })).unwrap_or_default(),
                            false,
                            None,
                            Some(err),
                        );
                    }
                };
                let bp_captured = full_utf8_capture(&raw_req_bytes);
                trace.req_size = raw_req_bytes.len();
                if recording {
                    trace.req_body = bp_captured.clone();
                }

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
                    req_body: bp_captured.clone(),
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
                            recording,
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
                        req_body: edited_body,
                        req_headers,
                    })) => {
                        if let Some(h) = req_headers {
                            eff_req_headers = h;
                            trace.req_headers = eff_req_headers.clone();
                        }
                        if let Some(b) = edited_body {
                            if Some(b.as_str()) != bp_captured.as_deref() {
                                let edited_bytes = Bytes::from(b.into_bytes());
                                trace.req_size = edited_bytes.len();
                                if recording {
                                    trace.req_body = full_utf8_capture(&edited_bytes);
                                }
                                breakpoint_forward_body = Some(edited_bytes);
                            } else {
                                breakpoint_forward_body = Some(raw_req_bytes);
                            }
                        } else {
                            breakpoint_forward_body = Some(raw_req_bytes);
                        }
                    }
                    Ok(Ok(BreakpointDecision::Abort { message })) => {
                        let msg = message.unwrap_or_else(|| "aborted at breakpoint".to_string());
                        return finalize_and_respond(
                            &state,
                            recording,
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
                        breakpoint_forward_body = Some(raw_req_bytes);
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
            if let Some(body) = req_body_opt.take() {
                if let Ok((sz, bd)) = drain_body_for_trace(body, recording).await {
                    trace.req_size = sz;
                    trace.req_body = bd;
                }
            }
            let mut headers = json_headers();
            headers.insert("x-oc-proxy-mocked".into(), "shadow".into());
            return finalize_and_respond(
                &state,
                recording,
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
        let mut offline_req_body = trace.req_body.clone();
        if let Some(body) = req_body_opt.take() {
            match drain_body_for_trace(body, true).await {
                Ok((sz, bd)) => {
                    trace.req_size = sz;
                    if recording {
                        trace.req_body = bd.clone();
                    }
                    offline_req_body = bd;
                }
                Err(err) => {
                    return finalize_and_respond(
                        &state,
                        recording,
                        &mut trace,
                        400,
                        json_headers(),
                        serde_json::to_vec(&json!({ "error": err })).unwrap_or_default(),
                        false,
                        None,
                        Some(err),
                    );
                }
            }
        }
        if let Some((status, val)) = serve_offline_fallback(
            &state,
            &method,
            &path,
            directory.as_deref(),
            offline_req_body.as_deref(),
        ) {
            let mut headers = json_headers();
            headers.insert("x-oc-proxy-mocked".into(), "offline".into());
            return finalize_and_respond(
                &state,
                recording,
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
            recording,
            &mut trace,
            404,
            json_headers(),
            serde_json::to_vec(&json!({ "name": "NotFoundError", "data": { "message": format!("No upstream configured and no offline handler for {} {}", method, path) } })).unwrap_or_default(),
            true,
            Some("offline_404".into()),
            None,
        );
    };

    // 4. Forward to Upstream OpenCode Server (streaming request/response with bounded tee capture)
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
            "host"
                | "connection"
                | "keep-alive"
                | "transfer-encoding"
                | "content-length"
                | "accept-encoding"
        ) {
            continue;
        }
        builder = builder.header(k.as_str(), v.as_str());
    }

    let mut req_capture_state: Option<Arc<Mutex<BodyCapture>>> = None;
    let outgoing_body = if let Some(bp_bytes) = breakpoint_forward_body {
        if bp_bytes.is_empty() {
            OutgoingReqBody::Empty
        } else {
            OutgoingReqBody::Full(bp_bytes)
        }
    } else if let Some(live_body) = req_body_opt.take() {
        let cap = Arc::new(Mutex::new(BodyCapture::new(recording)));
        match prepare_streaming_req_body(live_body, Arc::clone(&cap)).await {
            Ok(out) => {
                req_capture_state = Some(cap);
                out
            }
            Err(err) => {
                let (sz, bd) = cap.lock().unwrap().snapshot_size_and_body();
                trace.req_size = sz;
                if recording {
                    trace.req_body = bd;
                }
                return finalize_and_respond(
                    &state,
                    recording,
                    &mut trace,
                    400,
                    json_headers(),
                    serde_json::to_vec(&json!({ "error": err })).unwrap_or_default(),
                    false,
                    None,
                    Some(err),
                );
            }
        }
    } else {
        OutgoingReqBody::Empty
    };

    builder = match outgoing_body {
        OutgoingReqBody::Empty => builder,
        OutgoingReqBody::Full(b) => builder.body(b),
        OutgoingReqBody::Streaming(s) => builder.body(s),
    };

    match builder.send().await {
        Ok(res) => {
            if let Some(ref rc) = req_capture_state {
                let (sz, bd) = rc.lock().unwrap().snapshot_size_and_body();
                trace.req_size = sz;
                if recording {
                    trace.req_body = bd;
                }
            }

            let status = res.status().as_u16();
            let needs_shadow_merge = method == Method::GET
                && mount_shadow
                && status == 200
                && matches!(
                    path.trim_end_matches('/'),
                    "/session" | "/session/status" | "/permission" | "/question"
                )
                && !state.shadow.read().unwrap().sessions.is_empty();
            let needs_response_buffer = patch_to_apply.is_some() || needs_shadow_merge;

            let mut res_headers = BTreeMap::new();
            for (k, v) in res.headers() {
                let name = k.as_str().to_lowercase();
                if matches!(name.as_str(), "connection" | "keep-alive" | "transfer-encoding")
                    || (needs_response_buffer && name == "content-length")
                {
                    continue;
                }
                if let Ok(val) = v.to_str() {
                    res_headers.insert(name, val.to_string());
                }
            }

            if !needs_response_buffer {
                let extract_session_body =
                    session_id.is_none() && path.trim_end_matches('/') == "/session";
                trace.status = Some(status);
                trace.res_headers = res_headers.clone();

                let finalizer = StreamTraceFinalizer {
                    state: Arc::clone(&state),
                    recording,
                    trace,
                    req_capture: req_capture_state,
                    res_capture: BodyCapture::new(recording || extract_session_body),
                    extract_session_body,
                    finished: false,
                };
                let trace_id_header = finalizer.trace.id.clone();

                let body_stream = futures_util::stream::unfold(
                    (res.bytes_stream(), finalizer),
                    |(mut up_stream, mut fin)| async move {
                        match up_stream.next().await {
                            Some(Ok(chunk)) => {
                                fin.res_capture.observe_chunk(&chunk);
                                Some((Ok::<Bytes, std::io::Error>(chunk), (up_stream, fin)))
                            }
                            Some(Err(err)) => {
                                let msg = format!("upstream stream read error: {err}");
                                fin.finish(Some(msg));
                                Some((
                                    Err(std::io::Error::new(
                                        std::io::ErrorKind::Other,
                                        err.to_string(),
                                    )),
                                    (up_stream, fin),
                                ))
                            }
                            None => {
                                fin.finish(None);
                                None
                            }
                        }
                    },
                );

                let mut resp_builder = Response::builder().status(status);
                for (k, v) in res_headers {
                    if let (Ok(hn), Ok(hv)) = (
                        HeaderName::from_bytes(k.as_bytes()),
                        HeaderValue::from_str(&v),
                    ) {
                        resp_builder = resp_builder.header(hn, hv);
                    }
                }
                if let Ok(hv) = HeaderValue::from_str(&trace_id_header) {
                    resp_builder = resp_builder.header("x-oc-proxy-trace-id", hv);
                }
                return resp_builder
                    .body(Body::from_stream(body_stream))
                    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
            }

            let raw_bytes = match res.bytes().await {
                Ok(b) => b.to_vec(),
                Err(err) => {
                    let msg = format!("upstream read error: {err}");
                    return finalize_and_respond(
                        &state,
                        recording,
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
            if needs_shadow_merge {
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
                recording,
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
            if let Some(ref rc) = req_capture_state {
                let (sz, bd) = rc.lock().unwrap().snapshot_size_and_body();
                trace.req_size = sz;
                if recording {
                    trace.req_body = bd;
                }
            }
            let msg = format!("upstream connect error: {err}");
            finalize_and_respond(
                &state,
                recording,
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

#[allow(clippy::too_many_arguments)]
fn finalize_and_respond(
    state: &Arc<AppState>,
    recording: bool,
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
    trace.res_body = if recording {
        full_utf8_capture(&body_bytes)
    } else {
        None
    };
    trace.mocked = mocked;
    trace.mock_rule_id = mock_rule_id;
    trace.upstream_error = upstream_error;
    trace.pending = false;

    if recording {
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
