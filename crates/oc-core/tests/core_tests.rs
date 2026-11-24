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
    assert!(path_matches(
        "regex:^/session/ses_[0-9]+/(message|todo)$",
        "/session/ses_42/todo"
    ));
    assert!(!path_matches(
        "regex:^/session/ses_[0-9]+/(message|todo)$",
        "/session/ses_abc/todo"
    ));
    assert!(pattern_matches("message.part.*", "message.part.delta"));
    assert!(substring_or_regex_matches(r#"regex:"tool"\s*:\s*"bash""#, r#"{"tool": "bash"}"#));
}

#[test]
fn dynamic_http_and_sse_rules_with_one_shot_and_dropped_frames() {
    let store = Storage::open_in_memory().unwrap();
    let http_rule = MockRule {
        id: "rule_dyn_1".into(),
        name: "Dynamic Body & Query Match".into(),
        enabled: true,
        priority: 10,
        method: "POST".into(),
        path_pattern: "/session/:id/message".into(),
        query_pattern: Some("directory=*demo*".into()),
        body_contains: Some("trigger-mock".into()),
        directory_filter: None,
        action_type: MockActionType::Direct,
        breakpoint_phase: BreakpointPhase::Request,
        one_shot: true,
        hit_count: 0,
        status: 200,
        delay_ms: 0,
        headers: BTreeMap::new(),
        body: Some(json!({"ok": true})),
        patch: None,
        sse_sequence: Vec::new(),
        created_at: now_ms(),
    };
    store.upsert_mock_rule(&http_rule).unwrap();
    assert!(store
        .has_body_dependent_rule("POST", "/session/ses_1/message", Some("directory=/tmp/demo"), None)
        .unwrap());
    assert!(store
        .find_matching_rule_ext(
            "POST",
            "/session/ses_1/message",
            Some("directory=/tmp/demo"),
            None,
            Some("normal prompt"),
        )
        .unwrap()
        .is_none());
    let matched = store
        .find_matching_rule_ext(
            "POST",
            "/session/ses_1/message",
            Some("directory=/tmp/demo"),
            None,
            Some("please trigger-mock now"),
        )
        .unwrap()
        .expect("dynamic rule must match");
    assert_eq!(matched.id, "rule_dyn_1");

    // One-shot disables after 1 hit
    store.record_mock_rule_hit(&matched.id, matched.one_shot).unwrap();
    assert!(store
        .find_matching_rule_ext(
            "POST",
            "/session/ses_1/message",
            Some("directory=/tmp/demo"),
            None,
            Some("please trigger-mock now"),
        )
        .unwrap()
        .is_none());

    // SSE rule matching & dropped event exclusion from ShadowStore reconstruction
    let sse_rule = SseMockRule {
        id: "sserule_1".into(),
        name: "Drop bash perm".into(),
        enabled: true,
        priority: 10,
        event_type_pattern: "permission.*".into(),
        session_filter: Some("ses_1".into()),
        directory_filter: None,
        payload_contains: Some("rm -rf".into()),
        action_type: SseMockActionType::Drop,
        delay_ms: 0,
        patch: None,
        replace_events: Vec::new(),
        one_shot: false,
        hit_count: 0,
        created_at: now_ms(),
    };
    store.upsert_sse_mock_rule(&sse_rule).unwrap();
    assert!(store
        .find_matching_sse_rule(
            "permission.asked",
            Some("ses_1"),
            "/tmp/demo",
            r#"{"type":"permission.asked","cmd":"rm -rf"}"#,
        )
        .unwrap()
        .is_some());

    // Dropped SSE frame is stored for DevTools visibility but ignored by ShadowStore state reconstruction
    store
        .insert_sse_event_with_meta(
            now_ms(),
            SseSource::Upstream,
            "/tmp/demo",
            &json!({
                "type": "todo.updated",
                "properties": { "sessionID": "ses_1", "todos": [{ "id": "t1", "content": "should be dropped" }] }
            }),
            true,
            Some("sserule_1".into()),
            true,
        )
        .unwrap();
    let state = store.reconstruct_session_state("ses_1", None).unwrap();
    assert!(state.todos_for("ses_1").is_empty());
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
            mocked: false,
            mock_rule_id: None,
            dropped: false,
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
            mocked: false,
            mock_rule_id: None,
            dropped: false,
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
            mocked: false,
            mock_rule_id: None,
            dropped: false,
        },
    ];
    let folded = fold_sse_events(records);
    assert_eq!(folded.len(), 2);
}

#[test]
fn long_session_over_5000_events_is_not_truncated() {
    let store = Storage::open_in_memory().unwrap();
    let ts = now_ms();
    let sid = "ses_long_6000";

    store
        .insert_sse_event(
            ts,
            SseSource::Upstream,
            "/repo/long",
            &json!({
                "type": "session.created",
                "properties": {
                    "info": {
                        "id": sid,
                        "directory": "/repo/long",
                        "title": "Long Streaming Session"
                    }
                }
            }),
        )
        .unwrap();
    store
        .insert_sse_event(
            ts + 1,
            SseSource::Upstream,
            "/repo/long",
            &json!({
                "type": "message.updated",
                "properties": {
                    "info": { "id": "msg_long", "sessionID": sid, "role": "assistant" }
                }
            }),
        )
        .unwrap();
    store
        .insert_sse_event(
            ts + 2,
            SseSource::Upstream,
            "/repo/long",
            &json!({
                "type": "message.part.updated",
                "properties": {
                    "part": {
                        "id": "prt_long",
                        "sessionID": sid,
                        "messageID": "msg_long",
                        "type": "text",
                        "text": ""
                    }
                }
            }),
        )
        .unwrap();

    let delta_count = 5500usize;
    let mut seq_at_5200 = 0i64;
    for i in 1..=delta_count {
        let rec = store
            .insert_sse_event(
                ts + 10 + i as i64,
                SseSource::Upstream,
                "/repo/long",
                &json!({
                    "type": "message.part.delta",
                    "properties": {
                        "sessionID": sid,
                        "messageID": "msg_long",
                        "partID": "prt_long",
                        "field": "text",
                        "delta": "x"
                    }
                }),
            )
            .unwrap();
        if i == 5200 {
            seq_at_5200 = rec.seq;
        }
    }

    // Emit tail events well past the 5,000-row single-page query ceiling
    store
        .insert_sse_event(
            ts + 10_000,
            SseSource::Upstream,
            "/repo/long",
            &json!({
                "type": "todo.updated",
                "properties": {
                    "sessionID": sid,
                    "todos": [{ "id": "td_tail", "content": "Tail todo after 5500 deltas" }]
                }
            }),
        )
        .unwrap();
    store
        .insert_sse_event(
            ts + 10_001,
            SseSource::Upstream,
            "/repo/long",
            &json!({
                "type": "session.error",
                "properties": {
                    "sessionID": sid,
                    "error": "late_error_past_5000"
                }
            }),
        )
        .unwrap();

    // 1. Generic query no longer clamps explicit limits at 5,000
    let all_via_limit = store
        .list_sse_events(Some(sid), None, None, None, None, true, None, 99999, false)
        .unwrap();
    assert_eq!(all_via_limit.len(), 3 + delta_count + 2);

    // 2. Tape extraction includes all 5,505 session events without silent truncation
    let tape = store
        .extract_tape_from_session(sid, Some("Full Long Tape".into()))
        .unwrap();
    assert_eq!(tape.events.len(), 3 + delta_count + 2);

    // 3. Point-in-time state reconstruction works accurately beyond 5,000 events
    let mid_state = store
        .reconstruct_session_state(sid, Some(seq_at_5200))
        .unwrap();
    let mid_msgs = mid_state.messages_with_parts(sid);
    assert_eq!(
        mid_msgs[0]["parts"][0]["text"].as_str().unwrap().len(),
        5200
    );
    assert!(mid_state.todos_for(sid).is_empty());

    let full_state = store.reconstruct_session_state(sid, None).unwrap();
    let full_msgs = full_state.messages_with_parts(sid);
    assert_eq!(
        full_msgs[0]["parts"][0]["text"].as_str().unwrap().len(),
        delta_count
    );
    assert_eq!(full_state.todos_for(sid).len(), 1);

    // 4. Timeline delta-folding and anomaly detection also see events past 5,000
    let timeline = store.get_session_timeline(sid, true, false, 500).unwrap();
    assert!(timeline.iter().any(|item| matches!(
        item,
        TimelineItem::Sse { event, .. } if event.event_type == "todo.updated"
    )));

    let anomalies = find_anomalies(&store, &AnomalyOptions::default()).unwrap();
    assert!(anomalies.iter().any(|a| a.kind == "sse.session_error"));
}
