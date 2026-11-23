use oc_core::anomaly::{find_anomalies, AnomalyOptions};
use oc_core::openapi::{validate_tape_semantics, OpenApiRegistry};
use oc_core::shadow::{remap_event_directory, ShadowStore};
use oc_core::storage::fold_sse_events;
use oc_core::tape::{builtin_demo_tape, export_tape_jsonl, import_tape_jsonl};
use oc_core::*;
use serde_json::json;
use std::collections::BTreeMap;

#[test]
fn storage_http_sse_fts_and_delta_folding() {
    let store = Storage::open_in_memory().unwrap();
    let ts = now_ms();

    let mut trace = HttpTrace {
        id: "tr_1".into(),
        started_at: ts,
        completed_at: None,
        duration_ms: None,
        method: "POST".into(),
        path: "/session/ses_123/message".into(),
        query: Some("directory=%2Ftmp%2Fproj".into()),
        directory: Some("/tmp/proj".into()),
        session_id: Some("ses_123".into()),
        status: None,
        req_headers: BTreeMap::new(),
        req_body: Some(r#"{"parts":[{"type":"text","text":"hello proxy"}]}"#.into()),
        req_size: 48,
        res_headers: BTreeMap::new(),
        res_body: None,
        res_size: 0,
        mocked: false,
        mock_rule_id: None,
        upstream_error: None,
        pending: true,
    };
    store.insert_http_started(&trace).unwrap();
    trace.completed_at = Some(ts + 120);
    trace.duration_ms = Some(120);
    trace.status = Some(200);
    trace.res_body = Some(r#"{"info":{"id":"msg_1"}}"#.into());
    trace.res_size = 24;
    trace.pending = false;
    store.complete_http_trace(&trace).unwrap();

    // Emit part.updated followed by 3 consecutive deltas
    store
        .insert_sse_event(
            ts + 10,
            SseSource::Upstream,
            "/tmp/proj",
            &json!({
                "type": "message.part.updated",
                "properties": {
                    "part": { "id": "prt_1", "sessionID": "ses_123", "messageID": "msg_1", "type": "text", "text": "" }
                }
            }),
        )
        .unwrap();

    for (i, chunk) in ["Hello ", "OpenCode ", "world!"].iter().enumerate() {
        store
            .insert_sse_event(
                ts + 20 + (i as i64 * 10),
                SseSource::Upstream,
                "/tmp/proj",
                &json!({
                    "type": "message.part.delta",
                    "properties": {
                        "sessionID": "ses_123",
                        "messageID": "msg_1",
                        "partID": "prt_1",
                        "field": "text",
                        "delta": chunk
                    }
                }),
            )
            .unwrap();
    }

    let timeline = store.get_session_timeline("ses_123", true, false, 100).unwrap();
    // 1 HTTP + 1 part.updated + 1 FoldedDelta (3 deltas folded into 1!)
    assert_eq!(timeline.len(), 3);
    match &timeline[2] {
        TimelineItem::FoldedDelta {
            count,
            total_chars,
            preview,
            ..
        } => {
            assert_eq!(*count, 3);
            assert_eq!(*total_chars, "Hello OpenCode world!".len());
            assert_eq!(preview, "Hello OpenCode world!");
        }
        other => panic!("expected FoldedDelta, got {other:?}"),
    }

    let hits = store
        .search_traffic("OpenCode", Some("ses_123"), None, None, 20)
        .unwrap();
    assert!(!hits.is_empty());

    let sessions = store.list_sessions(None, false).unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].session_id, "ses_123");
    assert_eq!(sessions[0].sse_count, 4);
    assert_eq!(sessions[0].http_count, 1);
}

#[test]
fn shadow_store_and_demo_tape_roundtrip() {
    let tape = builtin_demo_tape("/workspace/original");
    validate_tape_semantics(&tape).expect("demo tape must be semantically valid");

    let jsonl = export_tape_jsonl(&tape).unwrap();
    let imported = import_tape_jsonl(&jsonl).unwrap();
    assert_eq!(imported.id, tape.id);
    assert_eq!(imported.events.len(), tape.events.len());

    let mut shadow = ShadowStore::new("/workspace/remapped");
    for ev in &imported.events {
        let remapped = remap_event_directory(&ev.payload, "/workspace/remapped");
        shadow.apply_event(&remapped);
    }

    let sess = shadow.session_by_id("ses_demo_replay_001").unwrap();
    assert_eq!(sess["directory"], "/workspace/remapped");

    let msgs = shadow.messages_with_parts("ses_demo_replay_001");
    assert_eq!(msgs.len(), 2);
    let asst_parts = msgs[0]["parts"].as_array().unwrap();
    // Verify text delta accumulation worked!
    let text_part = asst_parts
        .iter()
        .find(|p| p["id"] == "prt_demo_text_001")
        .unwrap();
    assert!(text_part["text"]
        .as_str()
        .unwrap()
        .contains("Found the root cause!"));
    assert_eq!(shadow.diffs_for("ses_demo_replay_001").len(), 1);
    assert_eq!(shadow.todos_for("ses_demo_replay_001").len(), 2);
}

#[test]
fn anomaly_detector_catches_orphan_delta_and_http_errors() {
    let store = Storage::open_in_memory().unwrap();
    let ts = now_ms();

    let trace = HttpTrace {
        id: "tr_err".into(),
        started_at: ts,
        completed_at: Some(ts + 50),
        duration_ms: Some(50),
        method: "GET".into(),
        path: "/session/ses_bad/message".into(),
        query: None,
        directory: Some("/proj".into()),
        session_id: Some("ses_bad".into()),
        status: Some(500),
        req_headers: BTreeMap::new(),
        req_body: None,
        req_size: 0,
        res_headers: BTreeMap::new(),
        res_body: Some(r#"{"name":"UnknownError"}"#.into()),
        res_size: 23,
        mocked: false,
        mock_rule_id: None,
        upstream_error: None,
        pending: false,
    };
    store.insert_http_started(&trace).unwrap();
    store.complete_http_trace(&trace).unwrap();

    // Emit delta WITHOUT prior part.updated
    store
        .insert_sse_event(
            ts + 10,
            SseSource::Upstream,
            "/proj",
            &json!({
                "type": "message.part.delta",
                "properties": {
                    "sessionID": "ses_bad",
                    "messageID": "msg_x",
                    "partID": "prt_missing",
                    "field": "text",
                    "delta": "dropped text"
                }
            }),
        )
        .unwrap();

    // Unanswered permission
    store
        .insert_sse_event(
            ts + 20,
            SseSource::Upstream,
            "/proj",
            &json!({
                "type": "permission.asked",
                "properties": {
                    "id": "perm_1",
                    "sessionID": "ses_bad",
                    "permission": "bash",
                    "patterns": ["rm -rf"],
                    "metadata": {},
                    "always": []
                }
            }),
        )
        .unwrap();

    let anomalies = find_anomalies(&store, &AnomalyOptions::default()).unwrap();
    let kinds: Vec<&str> = anomalies.iter().map(|a| a.kind.as_str()).collect();
    assert!(kinds.contains(&"http.error_status"));
    assert!(kinds.contains(&"sse.orphan_part_delta"));
    assert!(kinds.contains(&"sse.unanswered_permission"));
}

#[test]
fn openapi_faker_produces_schema_valid_payloads() {
    let reg = OpenApiRegistry::default();
    let summary = reg.summary();
    assert!(summary.operations.len() >= 10);
    assert!(summary.events.len() >= 10);

    let session_mock = reg
        .fake_http_response("GET", "/session/ses_999", "/my/repo")
        .unwrap();
    assert_eq!(session_mock["directory"], "/my/repo");

    let op = reg.find_operation("GET", "/session/ses_999").unwrap();
    reg.validate(op.response_schema.as_ref().unwrap(), &session_mock)
        .expect("faked session should pass jsonschema validation");

    let perm_event = reg.fake_sse_event("permission.asked", "/my/repo").unwrap();
    assert_eq!(perm_event["type"], "permission.asked");
    let ev_schema = reg.find_event_schema("permission.asked").unwrap();
    reg.validate(&ev_schema, &perm_event)
        .expect("faked SSE event should pass jsonschema validation");
}

#[test]
fn path_matching_rules_work() {
    assert!(path_matches("/session/:sessionID/message", "/session/ses_abc/message"));
    assert!(!path_matches("/session/:sessionID/message", "/session/ses_abc/todo"));
    assert!(path_matches("/session/*", "/session/ses_abc/message"));
    assert!(path_matches("*", "/anything/here"));
}

#[test]
fn fold_events_keeps_non_deltas_intact() {
    let records = vec![
        SseRecord {
            seq: 1,
            ts: 100,
            source: SseSource::Upstream,
            directory: "/d".into(),
            event_type: "session.status".into(),
            session_id: Some("s1".into()),
            message_id: None,
            part_id: None,
            payload: json!({"type":"session.status","properties":{"sessionID":"s1","status":{"type":"busy"}}}),
        },
        SseRecord {
            seq: 2,
            ts: 110,
            source: SseSource::Upstream,
            directory: "/d".into(),
            event_type: "message.part.delta".into(),
            session_id: Some("s1".into()),
            message_id: Some("m1".into()),
            part_id: Some("p1".into()),
            payload: json!({"type":"message.part.delta","properties":{"sessionID":"s1","messageID":"m1","partID":"p1","field":"text","delta":"A"}}),
        },
        SseRecord {
            seq: 3,
            ts: 120,
            source: SseSource::Upstream,
            directory: "/d".into(),
            event_type: "message.part.delta".into(),
            session_id: Some("s1".into()),
            message_id: Some("m1".into()),
            part_id: Some("p1".into()),
            payload: json!({"type":"message.part.delta","properties":{"sessionID":"s1","messageID":"m1","partID":"p1","field":"text","delta":"B"}}),
        },
    ];
    let folded = fold_sse_events(records);
    assert_eq!(folded.len(), 2);
}
