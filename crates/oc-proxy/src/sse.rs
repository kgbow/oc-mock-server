use crate::state::{AppState, OutgoingSseFrame, PendingBreakpointEntry, SseClientEntry, SseClientKind};
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::sse::{Event as AxumSseEvent, Sse};
use futures_util::StreamExt;
use oc_core::{
    extract_event_ids, json_merge_patch, now_ms, BreakpointDecision, BreakpointKind,
    ControlWsEvent, PendingBreakpoint, ProxyMode, SseMockActionType, SseSource, TemplateContext,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tokio_stream::wrappers::ReceiverStream;

/// OpenCode's GlobalSDKProvider enforces a 15s heartbeat watchdog (`HEARTBEAT_TIMEOUT_MS = 15_000`).
/// Injecting an independent heartbeat every 9s ensures paused/stepped replays never disconnect.
const HEARTBEAT_INTERVAL_SECS: u64 = 9;

#[derive(Debug, Deserialize, Default)]
pub struct DirQuery {
    pub directory: Option<String>,
}

pub async fn handle_global_event(
    State(state): State<Arc<AppState>>,
) -> Sse<impl futures_util::Stream<Item = Result<AxumSseEvent, Infallible>>> {
    subscribe_client(state, SseClientKind::Global)
}

pub async fn handle_directory_event(
    State(state): State<Arc<AppState>>,
    Query(query): Query<DirQuery>,
    headers: HeaderMap,
) -> Sse<impl futures_util::Stream<Item = Result<AxumSseEvent, Infallible>>> {
    let dir = query
        .directory
        .or_else(|| {
            headers
                .get("x-opencode-directory")
                .and_then(|v| v.to_str().ok())
                .map(ToOwned::to_owned)
        })
        .unwrap_or_else(|| state.primary_active_directory());
    state.record_directory(&dir);
    subscribe_client(state, SseClientKind::Directory(dir))
}

fn subscribe_client(
    state: Arc<AppState>,
    kind: SseClientKind,
) -> Sse<impl futures_util::Stream<Item = Result<AxumSseEvent, Infallible>>> {
    let client_id = uuid::Uuid::new_v4().to_string();
    let (tx, rx) = mpsc::channel::<OutgoingSseFrame>(512);

    // Send initial `server.connected` matching OpenCode server semantics
    let initial_data = match &kind {
        SseClientKind::Global => serde_json::to_string(&json!({
            "directory": "global",
            "payload": { "type": "server.connected", "properties": {} }
        }))
        .unwrap_or_default(),
        SseClientKind::Directory(_) => serde_json::to_string(&json!({
            "type": "server.connected",
            "properties": {}
        }))
        .unwrap_or_default(),
    };
    let _ = tx.try_send(OutgoingSseFrame { data: initial_data });

    let hb_data = match &kind {
        SseClientKind::Global => serde_json::to_string(&json!({
            "directory": "global",
            "payload": { "type": "server.heartbeat", "properties": {} }
        }))
        .unwrap_or_default(),
        SseClientKind::Directory(_) => serde_json::to_string(&json!({
            "type": "server.heartbeat",
            "properties": {}
        }))
        .unwrap_or_default(),
    };

    state.sse_clients.insert(
        client_id.clone(),
        SseClientEntry {
            kind,
            tx: tx.clone(),
        },
    );
    state.broadcast_status();

    // Independent per-client heartbeat + cleanup watchdog
    let state_hb = Arc::clone(&state);
    let cid_hb = client_id.clone();
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(HEARTBEAT_INTERVAL_SECS));
        ticker.tick().await;
        loop {
            ticker.tick().await;
            if tx
                .send(OutgoingSseFrame {
                    data: hb_data.clone(),
                })
                .await
                .is_err()
            {
                state_hb.sse_clients.remove(&cid_hb);
                state_hb.broadcast_status();
                break;
            }
        }
    });

    let stream = ReceiverStream::new(rx).map(|frame| {
        Ok::<AxumSseEvent, Infallible>(AxumSseEvent::default().data(frame.data))
    });

    Sse::new(stream)
}

/// Resilient background reader that ingests upstream OpenCode `/global/event` frames into the SSE Hub.
pub fn spawn_upstream_sse_reader(state: Arc<AppState>) {
    let Some(upstream) = state.config.upstream_url.clone() else {
        tracing::info!("running in offline mode (no upstream SSE configured)");
        return;
    };

    let url = format!("{}/global/event", upstream.trim_end_matches('/'));
    tokio::spawn(async move {
        let mut backoff = Duration::from_millis(500);
        loop {
            tracing::info!("connecting to upstream SSE: {url}");
            match state.http_client.get(&url).send().await {
                Ok(res) if res.status().is_success() => {
                    backoff = Duration::from_millis(500);
                    let mut stream = res.bytes_stream();
                    let mut buf = String::new();
                    let mut data_lines: Vec<String> = Vec::new();

                    while let Some(chunk_res) = stream.next().await {
                        let Ok(chunk) = chunk_res else {
                            break;
                        };
                        buf.push_str(&String::from_utf8_lossy(&chunk));

                        while let Some(pos) = buf.find('\n') {
                            let line = buf[..pos].trim_end_matches('\r').to_string();
                            buf.drain(..=pos);

                            if line.is_empty() {
                                if !data_lines.is_empty() {
                                    let joined = data_lines.join("\n");
                                    data_lines.clear();
                                    handle_upstream_sse_payload(&state, &joined);
                                }
                            } else if let Some(rest) = line.strip_prefix("data:") {
                                data_lines.push(rest.trim_start().to_string());
                            }
                        }
                    }
                    tracing::warn!("upstream SSE stream closed; reconnecting...");
                }
                Ok(res) => {
                    tracing::warn!("upstream SSE returned HTTP {}; retrying", res.status());
                }
                Err(err) => {
                    tracing::debug!("upstream SSE connect error: {err}");
                }
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(Duration::from_secs(5));
        }
    });
}

fn handle_upstream_sse_payload(state: &Arc<AppState>, raw: &str) {
    let Ok(parsed) = serde_json::from_str::<Value>(raw) else {
        return;
    };

    let (dir, payload) = if let Some(inner) = parsed.get("payload") {
        let d = parsed
            .get("directory")
            .and_then(Value::as_str)
            .unwrap_or("global")
            .to_string();
        (d, inner.clone())
    } else {
        ("global".to_string(), parsed)
    };

    let event_type = payload.get("type").and_then(Value::as_str).unwrap_or("");
    // Skip forwarding upstream heartbeat/connected since our hub manages per-client lifecycle
    if event_type == "server.heartbeat" || event_type == "server.connected" {
        return;
    }

    if dir != "global" && !dir.is_empty() {
        state.record_directory(&dir);
    }

    if state.is_muted() || state.mode() == ProxyMode::Replay {
        // Still record upstream events if recording is enabled, without forwarding to clients
        if state.is_recording() {
            if let Ok(rec) = state
                .storage
                .insert_sse_event(now_ms(), SseSource::Upstream, &dir, &payload)
            {
                state.broadcast(ControlWsEvent::SseRecorded { event: rec });
            }
        }
        return;
    }

    if state.rules_active() {
        let (session_id, _, _) = extract_event_ids(&payload);
        let raw_payload = serde_json::to_string(&payload).unwrap_or_default();
        if let Ok(Some(rule)) = state.storage.find_matching_sse_rule(
            event_type,
            session_id.as_deref(),
            &dir,
            &raw_payload,
        ) {
            let _ = state.storage.record_sse_rule_hit(&rule.id, rule.one_shot);
            state.broadcast(ControlWsEvent::SseRulesUpdated);

            let ctx = TemplateContext {
                session_id: session_id
                    .clone()
                    .unwrap_or_else(|| "ses_mock_001".to_string()),
                directory: dir.clone(),
                method: "SSE".to_string(),
                path: event_type.to_string(),
            };

            match rule.action_type {
                SseMockActionType::Drop => {
                    state.emit_sse_with_meta(
                        SseSource::Upstream,
                        &dir,
                        payload,
                        true,
                        Some(rule.id),
                        true,
                    );
                    return;
                }
                SseMockActionType::Delay => {
                    let state_bg = Arc::clone(state);
                    let rule_id = rule.id;
                    let delay_ms = rule.delay_ms;
                    tokio::spawn(async move {
                        if delay_ms > 0 {
                            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                        }
                        state_bg.emit_sse_with_meta(
                            SseSource::Upstream,
                            &dir,
                            payload,
                            true,
                            Some(rule_id),
                            false,
                        );
                    });
                    return;
                }
                SseMockActionType::Patch => {
                    let state_bg = Arc::clone(state);
                    let rule_id = rule.id;
                    let delay_ms = rule.delay_ms;
                    let patch_opt = rule.patch;
                    tokio::spawn(async move {
                        if delay_ms > 0 {
                            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                        }
                        let mut patched = payload;
                        if let Some(patch_val) = patch_opt {
                            let resolved_patch = ctx.apply_value(&patch_val);
                            json_merge_patch(&mut patched, &resolved_patch);
                        }
                        state_bg.emit_sse_with_meta(
                            SseSource::Upstream,
                            &dir,
                            patched,
                            true,
                            Some(rule_id),
                            false,
                        );
                    });
                    return;
                }
                SseMockActionType::Replace => {
                    // Record original upstream frame as suppressed/replaced for visibility
                    state.emit_sse_with_meta(
                        SseSource::Upstream,
                        &dir,
                        payload,
                        true,
                        Some(rule.id.clone()),
                        true,
                    );
                    let state_bg = Arc::clone(state);
                    let rule_id = rule.id;
                    let delay_ms = rule.delay_ms;
                    let mut steps = rule.replace_events;
                    if steps.is_empty() {
                        if let Some(single_val) = rule.patch {
                            steps.push(oc_core::MockSseStep {
                                delay_ms: 0,
                                payload: single_val,
                                directory: None,
                                event: None,
                                id: None,
                                apply_to_shadow: true,
                            });
                        }
                    }
                    tokio::spawn(async move {
                        if delay_ms > 0 {
                            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                        }
                        state_bg.spawn_sse_steps(rule_id, steps, ctx, SseSource::Injected);
                    });
                    return;
                }
                SseMockActionType::Breakpoint => {
                    let state_bg = Arc::clone(state);
                    let ev_type_str = event_type.to_string();
                    tokio::spawn(async move {
                        let bp_id =
                            format!("bp_{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
                        let (tx, rx) = oneshot::channel();
                        let bp = PendingBreakpoint {
                            id: bp_id.clone(),
                            kind: BreakpointKind::SseEvent,
                            trace_id: format!(
                                "sse_{}",
                                &uuid::Uuid::new_v4().simple().to_string()[..8]
                            ),
                            rule_id: rule.id.clone(),
                            rule_name: rule.name.clone(),
                            started_at: now_ms(),
                            method: "SSE".to_string(),
                            path: ev_type_str.clone(),
                            query: None,
                            directory: Some(dir.clone()),
                            req_headers: BTreeMap::new(),
                            req_body: None,
                            res_status: None,
                            res_headers: BTreeMap::new(),
                            res_body: None,
                            sse_event_type: Some(ev_type_str),
                            sse_session_id: session_id,
                            sse_payload: Some(payload.clone()),
                        };
                        state_bg.breakpoints.insert(
                            bp_id.clone(),
                            PendingBreakpointEntry {
                                info: bp.clone(),
                                tx,
                            },
                        );
                        state_bg.broadcast(ControlWsEvent::BreakpointAdded { breakpoint: bp });
                        state_bg.broadcast_status();

                        let decision = tokio::time::timeout(Duration::from_secs(300), rx).await;
                        state_bg.breakpoints.remove(&bp_id);
                        state_bg.broadcast(ControlWsEvent::BreakpointResolved { id: bp_id });
                        state_bg.broadcast_status();

                        match decision {
                            Ok(Ok(BreakpointDecision::EmitSse {
                                payload: edited,
                                directory: edited_dir,
                                apply_to_shadow,
                            })) => {
                                let target_dir = edited_dir
                                    .filter(|d| !d.trim().is_empty())
                                    .unwrap_or(dir);
                                if apply_to_shadow.unwrap_or(false) {
                                    state_bg.shadow.write().unwrap().apply_event(&edited);
                                }
                                state_bg.emit_sse_with_meta(
                                    SseSource::Upstream,
                                    &target_dir,
                                    edited,
                                    true,
                                    Some(rule.id),
                                    false,
                                );
                            }
                            Ok(Ok(BreakpointDecision::Drop))
                            | Ok(Ok(BreakpointDecision::Abort { .. })) => {
                                state_bg.emit_sse_with_meta(
                                    SseSource::Upstream,
                                    &dir,
                                    payload,
                                    true,
                                    Some(rule.id),
                                    true,
                                );
                            }
                            Ok(Ok(BreakpointDecision::Forward { req_body, .. })) => {
                                let out_payload = req_body
                                    .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                                    .unwrap_or(payload);
                                state_bg.emit_sse_with_meta(
                                    SseSource::Upstream,
                                    &dir,
                                    out_payload,
                                    true,
                                    Some(rule.id),
                                    false,
                                );
                            }
                            _ => {
                                state_bg.emit_sse(SseSource::Upstream, &dir, payload);
                            }
                        }
                    });
                    return;
                }
            }
        }
    }

    state.emit_sse(SseSource::Upstream, &dir, payload);
}
