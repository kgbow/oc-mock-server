use crate::state::{AppState, OutgoingSseFrame, SseClientEntry, SseClientKind};
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::sse::{Event as AxumSseEvent, Sse};
use futures_util::StreamExt;
use oc_core::{ProxyMode, SseSource};
use serde::Deserialize;
use serde_json::{json, Value};
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
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
                .insert_sse_event(oc_core::now_ms(), SseSource::Upstream, &dir, &payload)
            {
                state.broadcast(oc_core::ControlWsEvent::SseRecorded { event: rec });
            }
        }
        return;
    }

    state.emit_sse(SseSource::Upstream, &dir, payload);
}
