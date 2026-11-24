use axum::body::Bytes;
use axum::extract::DefaultBodyLimit;
use axum::response::sse::{Event as SseEvent, Sse};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::StreamExt;
use oc_core::*;
use oc_mcp::mcp_http_router;
use oc_proxy::{build_control_router, build_proxy_router, spawn_upstream_sse_reader, AppState, ProxyConfig};
use serde_json::{json, Value};
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

async fn spawn_fake_upstream() -> String {
    let (sse_push_tx, sse_push_rx) = tokio::sync::broadcast::channel::<String>(64);
    let app = Router::new()
        .route(
            "/global/health",
            get(|| async { Json(json!({ "healthy": true, "version": "1.2.3-upstream" })) }),
        )
        .route(
            "/session",
            get(|| async {
                Json(json!([
                    {
                        "id": "ses_real_1",
                        "slug": "real",
                        "projectID": "proj_1",
                        "directory": "/repo/live",
                        "title": "Real Upstream Session",
                        "version": "1.2.3",
                        "time": { "created": 1000, "updated": 1000 }
                    }
                ]))
            }),
        )
        .route(
            "/doc",
            get(|| async { Json(oc_core::openapi::builtin_opencode_openapi()) }),
        )
        .route(
            "/echo-large",
            post(|body: Bytes| async move { body }),
        )
        .route(
            "/emit-upstream-sse",
            post({
                let tx = sse_push_tx.clone();
                move |Json(ev): Json<Value>| {
                    let tx = tx.clone();
                    async move {
                        let _ = tx.send(ev.to_string());
                        Json(json!({ "pushed": true }))
                    }
                }
            }),
        )
        .layer(DefaultBodyLimit::disable())
        .route(
            "/global/event",
            get({
                let tx = sse_push_tx.clone();
                move || {
                    let rx = tx.subscribe();
                    async move {
                        let events = vec![
                            Ok::<SseEvent, Infallible>(SseEvent::default().data(
                                json!({
                                    "directory": "global",
                                    "payload": { "type": "server.connected", "properties": {} }
                                })
                                .to_string(),
                            )),
                            Ok::<SseEvent, Infallible>(SseEvent::default().data(
                                json!({
                                    "directory": "/repo/live",
                                    "payload": {
                                        "type": "session.status",
                                        "properties": { "sessionID": "ses_real_1", "status": { "type": "busy" } }
                                    }
                                })
                                .to_string(),
                            )),
                        ];
                        let live_stream = tokio_stream::wrappers::BroadcastStream::new(rx)
                            .filter_map(|res| async move {
                                res.ok().map(|s| Ok::<SseEvent, Infallible>(SseEvent::default().data(s)))
                            });
                        let stream = futures_util::stream::iter(events).chain(live_stream);
                        Sse::new(stream)
                    }
                }
            }),
        );
    drop(sse_push_rx);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn end_to_end_proxy_mock_breakpoint_replay_and_mcp() {
    let upstream = spawn_fake_upstream().await;
    let storage = Storage::open_in_memory().unwrap();

    let proxy_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let control_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_addr = proxy_listener.local_addr().unwrap().to_string();
    let control_addr = control_listener.local_addr().unwrap().to_string();

    let state = AppState::new(
        ProxyConfig {
            proxy_addr: proxy_addr.clone(),
            control_addr: control_addr.clone(),
            upstream_url: Some(upstream.clone()),
            default_directory: "/repo/live".into(),
            web_app_url: None,
        },
        storage,
    );

    spawn_upstream_sse_reader(Arc::clone(&state));
    let proxy_app = build_proxy_router(Arc::clone(&state));
    let control_app =
        build_control_router(Arc::clone(&state)).merge(mcp_http_router(Arc::clone(&state)));

    tokio::spawn(async move {
        let _ = axum::serve(proxy_listener, proxy_app).await;
    });
    tokio::spawn(async move {
        let _ = axum::serve(control_listener, control_app).await;
    });

    let client = reqwest::Client::new();
    let proxy_base = format!("http://{proxy_addr}");
    let control_base = format!("http://{control_addr}");

    // Allow upstream SSE reader to connect and ingest initial session.status event
    tokio::time::sleep(Duration::from_millis(200)).await;

    // 1. Transparent HTTP passthrough + trace recording
    let health: Value = client
        .get(format!("{proxy_base}/global/health"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(health["version"], "1.2.3-upstream");

    let traces: Vec<HttpTraceSummary> = client
        .get(format!("{control_base}/api/http"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(traces.iter().any(|t| t.path == "/global/health" && t.status == Some(200)));

    // 2. Sync OpenAPI /doc from upstream + generate deterministic fake response
    let oa: OpenApiSummary = client
        .post(format!("{control_base}/api/openapi/sync"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(oa.source.starts_with("upstream:"));
    assert!(!oa.events.is_empty());

    let faked: Value = client
        .post(format!("{control_base}/api/openapi/fake-http"))
        .json(&json!({ "method": "GET", "path": "/vcs" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(faked["branch"], "main");

    // 3. Direct Mock Rule + Manual Rule Update + Passthrough vs Intercept Mode Distinction
    let created_rule: MockRule = client
        .post(format!("{control_base}/api/rules"))
        .json(&json!({
            "name": "Mock VCS",
            "method": "GET",
            "path_pattern": "/vcs",
            "action_type": "direct",
            "status": 200,
            "body": { "branch": "feat/mocked-branch" }
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    // Creating an enabled rule auto-switches mode from Passthrough -> Intercept
    let st_after_rule: ProxyRuntimeStatus = client
        .get(format!("{control_base}/api/status"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(st_after_rule.mode, ProxyMode::Intercept);

    let vcs: Value = client
        .get(format!("{proxy_base}/vcs"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(vcs["branch"], "feat/mocked-branch");

    // Manual update of existing rule via PUT /api/rules/{id}
    client
        .put(format!("{control_base}/api/rules/{}", created_rule.id))
        .json(&json!({
            "name": "Mock VCS Edited",
            "method": "GET",
            "path_pattern": "/vcs",
            "action_type": "direct",
            "status": 200,
            "body": { "branch": "feat/manually-edited" }
        }))
        .send()
        .await
        .unwrap();
    let vcs_edited: Value = client
        .get(format!("{proxy_base}/vcs"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(vcs_edited["branch"], "feat/manually-edited");

    // Switching to Passthrough mode bypasses active rules cleanly without deleting them
    client
        .post(format!("{control_base}/api/mode"))
        .json(&json!({ "mode": "passthrough" }))
        .send()
        .await
        .unwrap();
    let passthrough_vcs_resp = client
        .get(format!("{proxy_base}/vcs"))
        .send()
        .await
        .unwrap();
    assert_eq!(passthrough_vcs_resp.status(), 404); // Fake upstream has no /vcs route!

    // Switching back to Intercept re-enables the rule immediately
    client
        .post(format!("{control_base}/api/mode"))
        .json(&json!({ "mode": "intercept" }))
        .send()
        .await
        .unwrap();
    let vcs_reintercepted: Value = client
        .get(format!("{proxy_base}/vcs"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(vcs_reintercepted["branch"], "feat/manually-edited");

    client
        .delete(format!("{control_base}/api/rules/{}", created_rule.id))
        .send()
        .await
        .unwrap();

    // 3b. HTTP Mock with Linked SSE Hub Sequence + Response-Phase Breakpoint + SSE Stream Interception
    let linked_rule: MockRule = client
        .post(format!("{control_base}/api/rules"))
        .json(&json!({
            "name": "Prompt triggers SSE sequence",
            "method": "POST",
            "path_pattern": "/session/:sessionID/message",
            "action_type": "direct",
            "status": 200,
            "body": { "accepted": true, "session": "{{session_id}}" },
            "sse_sequence": [
                {
                    "delay_ms": 20,
                    "apply_to_shadow": true,
                    "payload": {
                        "type": "todo.updated",
                        "properties": {
                            "sessionID": "{{session_id}}",
                            "todos": [{ "id": "td_linked", "content": "Triggered from HTTP mock", "status": "in_progress", "priority": "high" }]
                        }
                    }
                }
            ]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let prompt_resp: Value = client
        .post(format!("{proxy_base}/session/ses_linked_77/message?directory=/repo/live"))
        .json(&json!({ "text": "hi" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(prompt_resp["session"], "ses_linked_77");
    tokio::time::sleep(Duration::from_millis(80)).await;

    let linked_sse: Vec<SseRecord> = client
        .get(format!("{control_base}/api/sse?session_id=ses_linked_77"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(linked_sse.len(), 1);
    assert_eq!(linked_sse[0].event_type, "todo.updated");
    assert!(linked_sse[0].mocked);
    client
        .delete(format!("{control_base}/api/rules/{}", linked_rule.id))
        .send()
        .await
        .unwrap();

    // Response-stage HTTP Breakpoint: intercepts REAL upstream response and edits it before client receives it
    let res_bp_rule: MockRule = client
        .post(format!("{control_base}/api/rules"))
        .json(&json!({
            "name": "Response BP Health",
            "method": "GET",
            "path_pattern": "/global/health",
            "action_type": "breakpoint",
            "breakpoint_phase": "response",
            "one_shot": true,
            "priority": 1
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let client_res_bp = client.clone();
    let proxy_base_res_bp = proxy_base.clone();
    let res_inflight = tokio::spawn(async move {
        client_res_bp
            .get(format!("{proxy_base_res_bp}/global/health"))
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap()
    });
    tokio::time::sleep(Duration::from_millis(120)).await;
    let res_bps: Vec<PendingBreakpoint> = client
        .get(format!("{control_base}/api/breakpoints"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(res_bps.len(), 1);
    assert_eq!(res_bps[0].kind, BreakpointKind::HttpResponse);
    assert!(res_bps[0]
        .res_body
        .as_deref()
        .unwrap_or("")
        .contains("1.2.3-upstream"));

    client
        .post(format!("{control_base}/api/breakpoints/{}/resolve", res_bps[0].id))
        .json(&json!({
            "decision": "respond",
            "status": 200,
            "headers": { "content-type": "application/json" },
            "body": r#"{"healthy":true,"version":"edited-real-upstream-response"}"#
        }))
        .send()
        .await
        .unwrap();
    let edited_res = res_inflight.await.unwrap();
    assert_eq!(edited_res["version"], "edited-real-upstream-response");
    let _ = client
        .delete(format!("{control_base}/api/rules/{}", res_bp_rule.id))
        .send()
        .await;

    // Live Upstream SSE Patch Rule + Live SSE Breakpoint
    let sse_patch_rule: SseMockRule = client
        .post(format!("{control_base}/api/sse-rules"))
        .json(&json!({
            "name": "Patch permission command",
            "event_type_pattern": "permission.*",
            "payload_contains": "dangerous",
            "action_type": "patch",
            "one_shot": true,
            "patch": {
                "properties": { "patterns": ["safe-patched-cmd"] }
            }
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    client
        .post(format!("{upstream}/emit-upstream-sse"))
        .json(&json!({
            "directory": "/repo/live",
            "payload": {
                "type": "permission.asked",
                "properties": {
                    "id": "perm_live_1",
                    "sessionID": "ses_sse_test",
                    "permission": "bash",
                    "patterns": ["dangerous-cmd"]
                }
            }
        }))
        .send()
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(120)).await;

    let patched_sse_list: Vec<SseRecord> = client
        .get(format!("{control_base}/api/sse?session_id=ses_sse_test"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(patched_sse_list.len(), 1);
    assert_eq!(
        patched_sse_list[0].payload["properties"]["patterns"][0],
        "safe-patched-cmd"
    );
    assert!(patched_sse_list[0].mocked);
    let _ = client
        .delete(format!("{control_base}/api/sse-rules/{}", sse_patch_rule.id))
        .send()
        .await;

    // Live SSE Breakpoint: suspend upstream SSE frame, edit JSON manually, and forward
    let sse_bp_rule: SseMockRule = client
        .post(format!("{control_base}/api/sse-rules"))
        .json(&json!({
            "name": "BP on question.asked",
            "event_type_pattern": "question.asked",
            "action_type": "breakpoint",
            "one_shot": true
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    client
        .post(format!("{upstream}/emit-upstream-sse"))
        .json(&json!({
            "directory": "/repo/live",
            "payload": {
                "type": "question.asked",
                "properties": {
                    "id": "q_live_1",
                    "sessionID": "ses_sse_bp",
                    "header": "Original Question"
                }
            }
        }))
        .send()
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(120)).await;

    let sse_bps: Vec<PendingBreakpoint> = client
        .get(format!("{control_base}/api/breakpoints"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(sse_bps.len(), 1);
    assert_eq!(sse_bps[0].kind, BreakpointKind::SseEvent);
    client
        .post(format!("{control_base}/api/breakpoints/{}/resolve", sse_bps[0].id))
        .json(&json!({
            "decision": "emit_sse",
            "payload": {
                "type": "question.asked",
                "properties": {
                    "id": "q_live_1",
                    "sessionID": "ses_sse_bp",
                    "header": "Manually Edited SSE Question"
                }
            }
        }))
        .send()
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(80)).await;

    let bp_sse_records: Vec<SseRecord> = client
        .get(format!("{control_base}/api/sse?session_id=ses_sse_bp"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(bp_sse_records.len(), 1);
    assert_eq!(
        bp_sse_records[0].payload["properties"]["header"],
        "Manually Edited SSE Question"
    );
    let _ = client
        .delete(format!("{control_base}/api/sse-rules/{}", sse_bp_rule.id))
        .send()
        .await;

    // 4. Breakpoint suspend & resolve with custom response
    let bp_rule: MockRule = client
        .post(format!("{control_base}/api/rules"))
        .json(&json!({
            "name": "BP Health",
            "method": "GET",
            "path_pattern": "/global/health",
            "action_type": "breakpoint",
            "priority": 1
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let client_bp = client.clone();
    let proxy_base_bp = proxy_base.clone();
    let inflight = tokio::spawn(async move {
        client_bp
            .get(format!("{proxy_base_bp}/global/health"))
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap()
    });

    tokio::time::sleep(Duration::from_millis(120)).await;
    let bps: Vec<PendingBreakpoint> = client
        .get(format!("{control_base}/api/breakpoints"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(bps.len(), 1);
    client
        .post(format!("{control_base}/api/breakpoints/{}/resolve", bps[0].id))
        .json(&json!({
            "decision": "respond",
            "status": 200,
            "headers": { "content-type": "application/json" },
            "body": r#"{"healthy":true,"version":"from-breakpoint"}"#
        }))
        .send()
        .await
        .unwrap();
    let bp_res = inflight.await.unwrap();
    assert_eq!(bp_res["version"], "from-breakpoint");
    client
        .delete(format!("{control_base}/api/rules/{}", bp_rule.id))
        .send()
        .await
        .unwrap();

    // 5. Replay Engine + ShadowStore Hybrid Mount + Directory Remap + Seek
    let tapes: Vec<TapeSummary> = client
        .get(format!("{control_base}/api/tapes"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!tapes.is_empty());

    let loaded: ReplayStatus = client
        .post(format!("{control_base}/api/replay/load"))
        .json(&json!({
            "tape_id": tapes[0].id,
            "target_directory": "/repo/custom-client-dir",
            "mount_shadow": true,
            "speed": 20.0,
            "min_frame_ms": 5
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(loaded.cursor, 0);

    // Seek to end to project the full demo session into ShadowStore immediately
    let seeked: ReplayStatus = client
        .post(format!("{control_base}/api/replay/seek"))
        .json(&json!({ "cursor": loaded.total_events, "notify_clients": false }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(seeked.cursor, loaded.total_events);

    // Hybrid GET /session should merge the virtual replay session alongside upstream `ses_real_1`!
    let merged_sessions: Vec<Value> = client
        .get(format!("{proxy_base}/session"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(merged_sessions.iter().any(|s| s["id"] == "ses_real_1"));
    let replay_sess = merged_sessions
        .iter()
        .find(|s| s["id"] == "ses_demo_replay_001")
        .expect("shadow replay session must be merged into GET /session");
    assert_eq!(replay_sess["directory"], "/repo/custom-client-dir");

    // GET /session/ses_demo_replay_001/message served directly from ShadowStore
    let shadow_msgs: Vec<Value> = client
        .get(format!("{proxy_base}/session/ses_demo_replay_001/message"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(shadow_msgs.len(), 2);

    // 6. MCP Streamable HTTP (/mcp): initialize -> tools/list -> tools/call
    let init_res = client
        .post(format!("{control_base}/mcp"))
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": { "name": "test-agent", "version": "1.0" }
            }
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(init_res.status(), 200);
    let mcp_sid = init_res
        .headers()
        .get("mcp-session-id")
        .expect("initialize must return Mcp-Session-Id")
        .to_str()
        .unwrap()
        .to_string();

    let list_tools: Value = client
        .post(format!("{control_base}/mcp"))
        .header("mcp-session-id", &mcp_sid)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let tools = list_tools["result"]["tools"].as_array().unwrap();
    assert!(tools.len() >= 10);

    // Play the demo tape (fast) so events are recorded in SQLite, then query via MCP
    client
        .post(format!("{control_base}/api/replay/seek"))
        .json(&json!({ "cursor": 0, "notify_clients": false }))
        .send()
        .await
        .unwrap();
    client
        .post(format!("{control_base}/api/replay/play"))
        .send()
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(450)).await;

    let timeline_res: Value = client
        .post(format!("{control_base}/mcp"))
        .header("mcp-session-id", &mcp_sid)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "get_session_timeline",
                "arguments": { "session_id": "ses_demo_replay_001", "fold_deltas": true }
            }
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(timeline_res["result"]["isError"], false);
    let txt = timeline_res["result"]["content"][0]["text"].as_str().unwrap();
    assert!(txt.contains("folded_delta"), "timeline should fold consecutive deltas: {txt}");

    let state_res: Value = client
        .post(format!("{control_base}/mcp"))
        .header("mcp-session-id", &mcp_sid)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": {
                "name": "reconstruct_state_at",
                "arguments": { "session_id": "ses_demo_replay_001" }
            }
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(state_res["result"]["isError"], false);
    let state_txt = state_res["result"]["content"][0]["text"].as_str().unwrap();
    assert!(state_txt.contains("Found the root cause!"));

    // 7. Streaming HTTP bodies > 2 MiB succeed (no 413) and capture the FULL body without truncation
    let large_len = 3 * 1024 * 1024 + 17; // > 2 MiB
    let mut large_payload = vec![b'A'; large_len];
    large_payload[large_len - 8..].copy_from_slice(b"TAILMARK");

    let echo_res = client
        .post(format!("{proxy_base}/echo-large"))
        .body(large_payload.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(echo_res.status(), 200);
    let trace_id = echo_res
        .headers()
        .get("x-oc-proxy-trace-id")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let echoed_bytes = echo_res.bytes().await.unwrap();
    assert_eq!(echoed_bytes.len(), large_len);
    assert_eq!(&echoed_bytes[..], &large_payload[..]);

    let stored_trace: HttpTrace = client
        .get(format!("{control_base}/api/http/{trace_id}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(stored_trace.req_size, large_len);
    assert_eq!(stored_trace.res_size, large_len);
    assert_eq!(stored_trace.req_body.as_ref().unwrap().len(), large_len);
    assert_eq!(stored_trace.res_body.as_ref().unwrap().len(), large_len);
    assert!(stored_trace.req_body.as_ref().unwrap().ends_with("TAILMARK"));
    assert!(stored_trace.res_body.as_ref().unwrap().ends_with("TAILMARK"));

    // Disable recording and verify > 2 MiB request still streams cleanly without recording a trace
    client
        .post(format!("{control_base}/api/recording"))
        .json(&json!({ "enabled": false }))
        .send()
        .await
        .unwrap();
    let echo_unrecorded = client
        .post(format!("{proxy_base}/echo-large"))
        .body(large_payload.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(echo_unrecorded.status(), 200);
    let unrec_id = echo_unrecorded
        .headers()
        .get("x-oc-proxy-trace-id")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert_eq!(echo_unrecorded.bytes().await.unwrap().len(), large_len);
    let missing_trace = client
        .get(format!("{control_base}/api/http/{unrec_id}"))
        .send()
        .await
        .unwrap();
    assert_eq!(missing_trace.status(), 404);
}
