use crate::model::*;
use crate::storage::Storage;
use anyhow::Result;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
pub struct AnomalyOptions {
    pub session_id: Option<String>,
    pub exclude_session_id: Option<String>,
    pub slow_http_ms: i64,
    pub limit: usize,
}

impl Default for AnomalyOptions {
    fn default() -> Self {
        Self {
            session_id: None,
            exclude_session_id: None,
            slow_http_ms: 3000,
            limit: 100,
        }
    }
}

/// Scan captured HTTP traces and SSE events for common OpenCode C/S failure patterns.
pub fn find_anomalies(storage: &Storage, opts: &AnomalyOptions) -> Result<Vec<Anomaly>> {
    let mut anomalies = Vec::new();

    // 1. HTTP 4xx/5xx, upstream transport errors, and slow requests
    let traces = storage.list_all_http_traces(
        opts.session_id.as_deref(),
        None,
        None,
        opts.exclude_session_id.as_deref(),
    )?;

    for t in &traces {
        if let Some(err) = &t.upstream_error {
            anomalies.push(Anomaly {
                severity: AnomalySeverity::Error,
                kind: "http.upstream_error".into(),
                ts: t.started_at,
                session_id: t.session_id.clone(),
                ref_id: t.id.clone(),
                message: format!("{} {} failed with upstream error: {}", t.method, t.path, err),
                details: json!({ "method": t.method, "path": t.path, "error": err }),
            });
        } else if let Some(status) = t.status {
            if status >= 400 {
                anomalies.push(Anomaly {
                    severity: if status >= 500 {
                        AnomalySeverity::Error
                    } else {
                        AnomalySeverity::Warning
                    },
                    kind: "http.error_status".into(),
                    ts: t.started_at,
                    session_id: t.session_id.clone(),
                    ref_id: t.id.clone(),
                    message: format!("{} {} returned HTTP {}", t.method, t.path, status),
                    details: json!({ "method": t.method, "path": t.path, "status": status, "duration_ms": t.duration_ms }),
                });
            }
        }

        if let Some(dur) = t.duration_ms {
            // Long streaming prompt requests naturally take longer; flag non-prompt endpoints or extreme delays
            let is_prompt = t.path.ends_with("/message") && t.method == "POST";
            let threshold = if is_prompt {
                opts.slow_http_ms * 10
            } else {
                opts.slow_http_ms
            };
            if dur >= threshold {
                anomalies.push(Anomaly {
                    severity: AnomalySeverity::Warning,
                    kind: "http.slow_request".into(),
                    ts: t.started_at,
                    session_id: t.session_id.clone(),
                    ref_id: t.id.clone(),
                    message: format!("{} {} took {}ms (threshold {}ms)", t.method, t.path, dur, threshold),
                    details: json!({ "method": t.method, "path": t.path, "duration_ms": dur }),
                });
            }
        }
    }

    // 2. SSE anomalies: session.error, orphan deltas, reconnect storms, stuck prompts, dir mismatches
    let events = storage.list_all_sse_events(
        opts.session_id.as_deref(),
        None,
        None,
        None,
        None,
        true,
        opts.exclude_session_id.as_deref(),
    )?;

    let mut seen_parts: HashSet<(String, String)> = HashSet::new();
    let mut orphan_delta_reported: HashSet<(String, String)> = HashSet::new();
    let mut pending_permissions: HashMap<String, (i64, Option<String>, Value)> = HashMap::new();
    let mut pending_questions: HashMap<String, (i64, Option<String>, Value)> = HashMap::new();
    let mut session_dirs: HashMap<String, HashSet<String>> = HashMap::new();
    let mut connect_timestamps: Vec<(i64, i64)> = Vec::new();

    for ev in &events {
        if let Some(sid) = &ev.session_id {
            if !ev.directory.is_empty() && ev.directory != "global" {
                session_dirs
                    .entry(sid.clone())
                    .or_default()
                    .insert(ev.directory.clone());
            }
        }

        let props = ev.payload.get("properties").unwrap_or(&Value::Null);
        match ev.event_type.as_str() {
            "server.connected" => {
                connect_timestamps.push((ev.seq, ev.ts));
            }
            "session.error" => {
                anomalies.push(Anomaly {
                    severity: AnomalySeverity::Error,
                    kind: "sse.session_error".into(),
                    ts: ev.ts,
                    session_id: ev.session_id.clone(),
                    ref_id: format!("sse:{}", ev.seq),
                    message: format!(
                        "Session error event at seq #{}: {}",
                        ev.seq,
                        props.get("error").unwrap_or(props)
                    ),
                    details: ev.payload.clone(),
                });
            }
            "message.part.updated" => {
                if let (Some(mid), Some(pid)) = (&ev.message_id, &ev.part_id) {
                    seen_parts.insert((mid.clone(), pid.clone()));
                }
            }
            "message.part.delta" => {
                if let (Some(mid), Some(pid)) = (&ev.message_id, &ev.part_id) {
                    let key = (mid.clone(), pid.clone());
                    if !seen_parts.contains(&key) && orphan_delta_reported.insert(key) {
                        anomalies.push(Anomaly {
                            severity: AnomalySeverity::Error,
                            kind: "sse.orphan_part_delta".into(),
                            ts: ev.ts,
                            session_id: ev.session_id.clone(),
                            ref_id: format!("sse:{}", ev.seq),
                            message: format!(
                                "Orphan message.part.delta at seq #{} for message={} part={} (missing prior message.part.updated; OpenCode client will silently drop it)",
                                ev.seq, mid, pid
                            ),
                            details: json!({ "seq": ev.seq, "messageID": mid, "partID": pid }),
                        });
                    }
                }
            }
            "permission.asked" => {
                if let Some(id) = props.get("id").and_then(Value::as_str) {
                    pending_permissions.insert(
                        id.to_string(),
                        (ev.ts, ev.session_id.clone(), props.clone()),
                    );
                }
            }
            "permission.replied" => {
                if let Some(req_id) = props.get("requestID").and_then(Value::as_str) {
                    pending_permissions.remove(req_id);
                }
            }
            "question.asked" => {
                if let Some(id) = props.get("id").and_then(Value::as_str) {
                    pending_questions.insert(
                        id.to_string(),
                        (ev.ts, ev.session_id.clone(), props.clone()),
                    );
                }
            }
            "question.replied" | "question.rejected" => {
                if let Some(req_id) = props.get("requestID").and_then(Value::as_str) {
                    pending_questions.remove(req_id);
                }
            }
            _ => {}
        }
    }

    if connect_timestamps.len() >= 3 {
        let first = connect_timestamps.first().unwrap();
        let last = connect_timestamps.last().unwrap();
        if last.1 - first.1 <= 60_000 {
            anomalies.push(Anomaly {
                severity: AnomalySeverity::Warning,
                kind: "sse.reconnect_storm".into(),
                ts: last.1,
                session_id: opts.session_id.clone(),
                ref_id: format!("sse:{}", last.0),
                message: format!(
                    "Detected {} server.connected events within {}ms (possible heartbeat timeout or proxy disconnect loop)",
                    connect_timestamps.len(),
                    last.1 - first.1
                ),
                details: json!({ "count": connect_timestamps.len(), "window_ms": last.1 - first.1 }),
            });
        }
    }

    for (id, (ts, sid, props)) in pending_permissions {
        anomalies.push(Anomaly {
            severity: AnomalySeverity::Warning,
            kind: "sse.unanswered_permission".into(),
            ts,
            session_id: sid,
            ref_id: id.clone(),
            message: format!("Permission request {} was asked but has no permission.replied event", id),
            details: props,
        });
    }

    for (id, (ts, sid, props)) in pending_questions {
        anomalies.push(Anomaly {
            severity: AnomalySeverity::Warning,
            kind: "sse.unanswered_question".into(),
            ts,
            session_id: sid,
            ref_id: id.clone(),
            message: format!("Question request {} was asked but has no replied/rejected event", id),
            details: props,
        });
    }

    for (sid, dirs) in session_dirs {
        if dirs.len() > 1 {
            let list: Vec<String> = dirs.into_iter().collect();
            anomalies.push(Anomaly {
                severity: AnomalySeverity::Warning,
                kind: "sse.directory_mismatch".into(),
                ts: now_ms(),
                session_id: Some(sid.clone()),
                ref_id: sid.clone(),
                message: format!(
                    "Session {} emitted SSE events across multiple directories {:?} (client drops events not matching open directory)",
                    sid, list
                ),
                details: json!({ "sessionID": sid, "directories": list }),
            });
        }
    }

    anomalies.sort_by(|a, b| b.ts.cmp(&a.ts));
    if anomalies.len() > opts.limit {
        anomalies.truncate(opts.limit);
    }
    Ok(anomalies)
}
