use axum::response::sse::{Event as SseEvent, Sse};
use axum::routing::get;
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
            "/global/event",
            get(|| async {
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
                let stream = futures_util::stream::iter(events).chain(futures_util::stream::pending());
                Sse::new(stream)
            }),
        );

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

    // 3. Direct Mock Rule + JSON Patch Rule
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
    let vcs: Value = client
        .get(format!("{proxy_base}/vcs"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(vcs["branch"], "feat/mocked-branch");
    client
        .delete(format!("{control_base}/api/rules/{}", created_rule.id))
        .send()
        .await
        .unwrap();

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
}
