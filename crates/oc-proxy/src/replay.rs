use crate::state::AppState;
use anyhow::{anyhow, Result};
use oc_core::shadow::remap_event_directory;
use oc_core::*;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

pub fn load_tape(
    state: &Arc<AppState>,
    tape_id: &str,
    target_directory: Option<String>,
    mount_shadow: bool,
    speed: Option<f64>,
    min_frame_ms: Option<u64>,
) -> Result<ReplayStatus> {
    let tape = state
        .storage
        .get_tape(tape_id)?
        .ok_or_else(|| anyhow!("tape '{}' not found", tape_id))?;

    let target_dir = target_directory
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| state.primary_active_directory());

    let web_url = tape
        .session_id
        .as_deref()
        .map(|sid| state.build_web_session_url(&target_dir, sid));

    // Reset ShadowStore with initial snapshot
    {
        let mut shadow = state.shadow.write().unwrap();
        *shadow = ShadowStore::new(target_dir.clone());
        shadow.load_snapshot(&tape.snapshot, Some(&target_dir));
    }

    let status = {
        let mut rt = state.replay.lock().unwrap();
        if let Some(handle) = rt.task.take() {
            handle.abort();
        }
        let spd = speed.unwrap_or(rt.status.speed).clamp(0.1, 50.0);
        let min_ms = min_frame_ms.unwrap_or(rt.status.min_frame_ms);
        rt.status = ReplayStatus {
            state: ReplayStateKind::Paused,
            tape_id: Some(tape.id.clone()),
            tape_name: Some(tape.name.clone()),
            session_id: tape.session_id.clone(),
            cursor: 0,
            total_events: tape.events.len(),
            speed: spd,
            min_frame_ms: min_ms,
            target_directory: Some(target_dir),
            mount_shadow,
            web_session_url: web_url,
        };
        rt.tape = Some(tape);
        rt.status.clone()
    };

    state.broadcast(ControlWsEvent::ReplayUpdated {
        replay: status.clone(),
    });
    state.broadcast_status();
    Ok(status)
}

pub fn play(state: &Arc<AppState>) -> Result<ReplayStatus> {
    let status = {
        let mut rt = state.replay.lock().unwrap();
        let Some(tape) = rt.tape.clone() else {
            return Err(anyhow!("no tape loaded"));
        };
        if let Some(handle) = rt.task.take() {
            handle.abort();
        }
        if rt.status.cursor >= tape.events.len() {
            // Restart from beginning if already completed
            rt.status.cursor = 0;
            let target_dir = rt
                .status
                .target_directory
                .clone()
                .unwrap_or_else(|| state.primary_active_directory());
            let mut shadow = state.shadow.write().unwrap();
            *shadow = ShadowStore::new(target_dir.clone());
            shadow.load_snapshot(&tape.snapshot, Some(&target_dir));
        }

        rt.status.state = ReplayStateKind::Playing;
        let state_bg = Arc::clone(state);
        let join = tokio::spawn(async move {
            run_replay_loop(state_bg).await;
        });
        rt.task = Some(join.abort_handle());
        rt.status.clone()
    };

    state.broadcast(ControlWsEvent::ReplayUpdated {
        replay: status.clone(),
    });
    state.broadcast_status();
    Ok(status)
}

pub fn pause(state: &Arc<AppState>) -> ReplayStatus {
    let status = {
        let mut rt = state.replay.lock().unwrap();
        if let Some(handle) = rt.task.take() {
            handle.abort();
        }
        if rt.tape.is_some() && rt.status.state == ReplayStateKind::Playing {
            rt.status.state = ReplayStateKind::Paused;
        }
        rt.status.clone()
    };
    state.broadcast(ControlWsEvent::ReplayUpdated {
        replay: status.clone(),
    });
    state.broadcast_status();
    status
}

pub fn stop(state: &Arc<AppState>) -> ReplayStatus {
    let status = {
        let mut rt = state.replay.lock().unwrap();
        if let Some(handle) = rt.task.take() {
            handle.abort();
        }
        if let Some(tape) = &rt.tape {
            let target_dir = rt
                .status
                .target_directory
                .clone()
                .unwrap_or_else(|| state.primary_active_directory());
            let mut shadow = state.shadow.write().unwrap();
            *shadow = ShadowStore::new(target_dir.clone());
            shadow.load_snapshot(&tape.snapshot, Some(&target_dir));
        }
        rt.status.cursor = 0;
        rt.status.state = ReplayStateKind::Stopped;
        rt.status.clone()
    };
    state.broadcast(ControlWsEvent::ReplayUpdated {
        replay: status.clone(),
    });
    state.broadcast_status();
    status
}

pub fn step_next(state: &Arc<AppState>) -> Result<ReplayStatus> {
    let (ev_opt, target_dir, status) = {
        let mut rt = state.replay.lock().unwrap();
        if let Some(handle) = rt.task.take() {
            handle.abort();
        }
        let Some(tape) = &rt.tape else {
            return Err(anyhow!("no tape loaded"));
        };
        let total = tape.events.len();
        let target_dir = rt
            .status
            .target_directory
            .clone()
            .unwrap_or_else(|| state.primary_active_directory());

        if rt.status.cursor >= total {
            rt.status.state = ReplayStateKind::Completed;
            (None, target_dir, rt.status.clone())
        } else {
            let ev = tape.events[rt.status.cursor].clone();
            rt.status.cursor += 1;
            rt.status.state = if rt.status.cursor >= total {
                ReplayStateKind::Completed
            } else {
                ReplayStateKind::Paused
            };
            (Some(ev), target_dir, rt.status.clone())
        }
    };

    if let Some(ev) = ev_opt {
        apply_and_emit_replay_event(state, &ev, &target_dir);
    }

    state.broadcast(ControlWsEvent::ReplayUpdated {
        replay: status.clone(),
    });
    state.broadcast_status();
    Ok(status)
}

/// Seek replay cursor to `target_cursor`: deterministically rebuilds ShadowStore up to
/// `target_cursor` and optionally triggers a client refresh via `server.connected`.
pub fn seek(state: &Arc<AppState>, target_cursor: usize, notify_clients: bool) -> Result<ReplayStatus> {
    let status = {
        let mut rt = state.replay.lock().unwrap();
        let was_playing = rt.status.state == ReplayStateKind::Playing;
        if let Some(handle) = rt.task.take() {
            handle.abort();
        }
        let Some(tape) = rt.tape.clone() else {
            return Err(anyhow!("no tape loaded"));
        };
        let clamped = target_cursor.min(tape.events.len());
        let target_dir = rt
            .status
            .target_directory
            .clone()
            .unwrap_or_else(|| state.primary_active_directory());

        {
            let mut shadow = state.shadow.write().unwrap();
            *shadow = ShadowStore::new(target_dir.clone());
            shadow.load_snapshot(&tape.snapshot, Some(&target_dir));
            for ev in &tape.events[..clamped] {
                let remapped = remap_event_directory(&ev.payload, &target_dir);
                shadow.apply_event(&remapped);
            }
        }

        rt.status.cursor = clamped;
        rt.status.state = if clamped >= tape.events.len() {
            ReplayStateKind::Completed
        } else if was_playing {
            ReplayStateKind::Playing
        } else {
            ReplayStateKind::Paused
        };

        if rt.status.state == ReplayStateKind::Playing {
            let state_bg = Arc::clone(state);
            let join = tokio::spawn(async move {
                run_replay_loop(state_bg).await;
            });
            rt.task = Some(join.abort_handle());
        }
        rt.status.clone()
    };

    if notify_clients {
        state.broadcast_raw_sse(
            "global",
            json!({ "type": "server.connected", "properties": {} }),
        );
    }

    state.broadcast(ControlWsEvent::ReplayUpdated {
        replay: status.clone(),
    });
    state.broadcast_status();
    Ok(status)
}

pub fn set_speed(state: &Arc<AppState>, speed: f64, min_frame_ms: Option<u64>) -> ReplayStatus {
    let status = {
        let mut rt = state.replay.lock().unwrap();
        rt.status.speed = speed.clamp(0.1, 50.0);
        if let Some(m) = min_frame_ms {
            rt.status.min_frame_ms = m;
        }
        rt.status.clone()
    };
    state.broadcast(ControlWsEvent::ReplayUpdated {
        replay: status.clone(),
    });
    status
}

/// Inject OpenCode's native `tui.session.select` SSE event so attached TUI clients automatically
/// open the replay session details view.
pub fn select_in_tui(state: &Arc<AppState>, session_id: Option<String>) -> Result<Value> {
    let (sid, dir) = {
        let rt = state.replay.lock().unwrap();
        let s = session_id
            .or_else(|| rt.status.session_id.clone())
            .ok_or_else(|| anyhow!("no session_id provided or loaded in replay"))?;
        let d = rt
            .status
            .target_directory
            .clone()
            .unwrap_or_else(|| state.primary_active_directory());
        (s, d)
    };

    let payload = json!({
        "type": "tui.session.select",
        "properties": { "sessionID": sid }
    });
    state.emit_sse(SseSource::Injected, &dir, payload.clone());
    Ok(payload)
}

async fn run_replay_loop(state: Arc<AppState>) {
    loop {
        let step = {
            let rt = state.replay.lock().unwrap();
            if rt.status.state != ReplayStateKind::Playing {
                return;
            }
            let Some(tape) = &rt.tape else {
                return;
            };
            let idx = rt.status.cursor;
            if idx >= tape.events.len() {
                None
            } else {
                let ev = tape.events[idx].clone();
                let prev_offset = if idx > 0 {
                    tape.events[idx - 1].offset_ms
                } else {
                    0
                };
                let raw_delta = ev.offset_ms.saturating_sub(prev_offset);
                let scaled = ((raw_delta as f64) / rt.status.speed).round() as u64;
                let delay_ms = if idx == 0 {
                    0
                } else {
                    scaled.max(rt.status.min_frame_ms)
                };
                let target_dir = rt
                    .status
                    .target_directory
                    .clone()
                    .unwrap_or_else(|| state.primary_active_directory());
                Some((ev, delay_ms, target_dir))
            }
        };

        let Some((ev, delay_ms, target_dir)) = step else {
            let status = {
                let mut rt = state.replay.lock().unwrap();
                rt.status.state = ReplayStateKind::Completed;
                rt.task = None;
                rt.status.clone()
            };
            state.broadcast(ControlWsEvent::ReplayUpdated {
                replay: status.clone(),
            });
            state.broadcast_status();
            return;
        };

        if delay_ms > 0 {
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        }

        // Re-check playing state after sleep
        let status = {
            let mut rt = state.replay.lock().unwrap();
            if rt.status.state != ReplayStateKind::Playing {
                return;
            }
            rt.status.cursor += 1;
            if rt.status.cursor >= rt.status.total_events {
                rt.status.state = ReplayStateKind::Completed;
                rt.task = None;
            }
            rt.status.clone()
        };

        apply_and_emit_replay_event(&state, &ev, &target_dir);
        state.broadcast(ControlWsEvent::ReplayUpdated {
            replay: status.clone(),
        });

        if status.state == ReplayStateKind::Completed {
            state.broadcast_status();
            return;
        }
    }
}

fn apply_and_emit_replay_event(state: &Arc<AppState>, ev: &TapeEvent, target_dir: &str) {
    let dir = if ev.directory == "global" {
        "global"
    } else {
        target_dir
    };
    let remapped = remap_event_directory(&ev.payload, target_dir);
    {
        let mut shadow = state.shadow.write().unwrap();
        shadow.apply_event(&remapped);
    }
    state.emit_sse(SseSource::Replay, dir, remapped);
}
