use crate::replay;
use crate::state::AppState;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use oc_core::anomaly::{find_anomalies, AnomalyOptions};
use oc_core::llm::{generate_http_mock, generate_sse_scenario, mutate_payload};
use oc_core::openapi::OpenApiRegistry;
use oc_core::storage::fold_sse_events;
use oc_core::tape::{export_tape_jsonl, import_tape_jsonl};
use oc_core::*;
use rust_embed::RustEmbed;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};

#[derive(RustEmbed)]
#[folder = "../../web/dist"]
struct WebAssets;

pub fn build_control_router(state: Arc<AppState>) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    Router::new()
        .route("/api/status", get(get_status))
        .route("/api/mode", post(set_mode))
        .route("/api/mute-sse", post(set_mute_sse))
        .route("/api/recording", post(set_recording))
        .route("/api/traffic", delete(clear_traffic))
        .route("/api/http", get(list_http))
        .route("/api/http/{id}", get(get_http))
        .route("/api/sse", get(list_sse))
        .route("/api/sse/inject", post(inject_sse))
        .route("/api/sessions", get(list_sessions))
        .route("/api/sessions/{id}/timeline", get(get_session_timeline))
        .route("/api/sessions/{id}/ignore", post(set_session_ignore))
        .route("/api/sessions/{id}/state", get(get_session_state))
        .route("/api/search", get(search_traffic))
        .route("/api/anomalies", get(get_anomalies))
        .route("/api/rules", get(list_rules).post(create_rule))
        .route("/api/rules/{id}", put(update_rule).delete(delete_rule))
        .route("/api/breakpoints", get(list_breakpoints))
        .route("/api/breakpoints/{id}/resolve", post(resolve_breakpoint))
        .route("/api/tapes", get(list_tapes).post(save_tape))
        .route("/api/tapes/extract/{session_id}", post(extract_tape))
        .route("/api/tapes/import", post(import_tape))
        .route("/api/tapes/{id}", get(get_tape).delete(delete_tape))
        .route("/api/tapes/{id}/export", get(export_tape))
        .route("/api/replay/status", get(get_replay_status))
        .route("/api/replay/load", post(replay_load))
        .route("/api/replay/play", post(replay_play))
        .route("/api/replay/pause", post(replay_pause))
        .route("/api/replay/stop", post(replay_stop))
        .route("/api/replay/step", post(replay_step))
        .route("/api/replay/seek", post(replay_seek))
        .route("/api/replay/speed", post(replay_speed))
        .route("/api/replay/tui-select", post(replay_tui_select))
        .route("/api/openapi", get(get_openapi))
        .route("/api/openapi/sync", post(sync_openapi))
        .route("/api/openapi/fake-http", post(openapi_fake_http))
        .route("/api/openapi/fake-sse", post(openapi_fake_sse))
        .route("/api/llm/config", get(get_llm_config).post(set_llm_config))
        .route("/api/llm/generate-http", post(llm_generate_http))
        .route("/api/llm/generate-scenario", post(llm_generate_scenario))
        .route("/api/llm/mutate", post(llm_mutate))
        .route("/api/ws", get(handle_ws))
        .fallback(serve_web_ui)
        .layer(DefaultBodyLimit::disable())
        .layer(cors)
        .with_state(state)
}

fn api_err(status: StatusCode, msg: impl ToString) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "error": msg.to_string() })))
}

async fn get_status(State(state): State<Arc<AppState>>) -> Json<ProxyRuntimeStatus> {
    Json(state.runtime_status())
}

#[derive(Deserialize)]
struct ModeBody {
    mode: ProxyMode,
}

async fn set_mode(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ModeBody>,
) -> Json<ProxyRuntimeStatus> {
    state.set_mode(body.mode);
    Json(state.runtime_status())
}

#[derive(Deserialize)]
struct BoolBody {
    enabled: bool,
}

async fn set_mute_sse(
    State(state): State<Arc<AppState>>,
    Json(body): Json<BoolBody>,
) -> Json<ProxyRuntimeStatus> {
    state.set_muted(body.enabled);
    Json(state.runtime_status())
}

async fn set_recording(
    State(state): State<Arc<AppState>>,
    Json(body): Json<BoolBody>,
) -> Json<ProxyRuntimeStatus> {
    state.set_recording(body.enabled);
    Json(state.runtime_status())
}

async fn clear_traffic(State(state): State<Arc<AppState>>) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    state
        .storage
        .clear_traffic()
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(json!({ "cleared": true })))
}

#[derive(Deserialize, Default)]
struct HttpListQuery {
    session_id: Option<String>,
    exclude_session_id: Option<String>,
    method: Option<String>,
    status_min: Option<u16>,
    limit: Option<usize>,
    offset: Option<usize>,
}

async fn list_http(
    State(state): State<Arc<AppState>>,
    Query(q): Query<HttpListQuery>,
) -> Result<Json<Vec<HttpTraceSummary>>, (StatusCode, Json<Value>)> {
    let items = state
        .storage
        .list_http_traces(
            q.session_id.as_deref(),
            q.method.as_deref(),
            q.status_min,
            q.exclude_session_id.as_deref(),
            q.limit.unwrap_or(200),
            q.offset.unwrap_or(0),
        )
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(items))
}

async fn get_http(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<HttpTrace>, (StatusCode, Json<Value>)> {
    let item = state
        .storage
        .get_http_trace(&id)
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e))?
        .ok_or_else(|| api_err(StatusCode::NOT_FOUND, "trace not found"))?;
    Ok(Json(item))
}

#[derive(Deserialize, Default)]
struct SseListQuery {
    session_id: Option<String>,
    exclude_session_id: Option<String>,
    event_type: Option<String>,
    directory: Option<String>,
    from_seq: Option<i64>,
    to_seq: Option<i64>,
    include_heartbeat: Option<bool>,
    fold: Option<bool>,
    limit: Option<usize>,
}

async fn list_sse(
    State(state): State<Arc<AppState>>,
    Query(q): Query<SseListQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let descending = q.from_seq.is_none() && q.to_seq.is_none();
    let mut events = state
        .storage
        .list_sse_events(
            q.session_id.as_deref(),
            q.event_type.as_deref(),
            q.directory.as_deref(),
            q.from_seq,
            q.to_seq,
            !q.include_heartbeat.unwrap_or(false),
            q.exclude_session_id.as_deref(),
            q.limit.unwrap_or(300),
            descending,
        )
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e))?;

    if q.fold.unwrap_or(false) {
        if descending {
            events.reverse();
        }
        let folded = fold_sse_events(events);
        Ok(Json(serde_json::to_value(folded).unwrap_or(Value::Null)))
    } else {
        Ok(Json(serde_json::to_value(events).unwrap_or(Value::Null)))
    }
}

#[derive(Deserialize)]
struct InjectSseBody {
    directory: Option<String>,
    payload: Value,
    #[serde(default)]
    apply_to_shadow: bool,
}

async fn inject_sse(
    State(state): State<Arc<AppState>>,
    Json(body): Json<InjectSseBody>,
) -> Json<Value> {
    let dir = body
        .directory
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| state.primary_active_directory());
    if body.apply_to_shadow {
        state.shadow.write().unwrap().apply_event(&body.payload);
    }
    let rec = state.emit_sse(SseSource::Injected, &dir, body.payload);
    Json(json!({ "ok": true, "recorded": rec }))
}

#[derive(Deserialize, Default)]
struct SessionsQuery {
    exclude_session_id: Option<String>,
    include_ignored: Option<bool>,
}

async fn list_sessions(
    State(state): State<Arc<AppState>>,
    Query(q): Query<SessionsQuery>,
) -> Result<Json<Vec<SessionOverview>>, (StatusCode, Json<Value>)> {
    let list = state
        .storage
        .list_sessions(
            q.exclude_session_id.as_deref(),
            q.include_ignored.unwrap_or(true),
        )
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(list))
}

#[derive(Deserialize, Default)]
struct TimelineQuery {
    fold: Option<bool>,
    include_heartbeat: Option<bool>,
    limit: Option<usize>,
}

async fn get_session_timeline(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(q): Query<TimelineQuery>,
) -> Result<Json<Vec<TimelineItem>>, (StatusCode, Json<Value>)> {
    let items = state
        .storage
        .get_session_timeline(
            &id,
            q.fold.unwrap_or(true),
            q.include_heartbeat.unwrap_or(false),
            q.limit.unwrap_or(400),
        )
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(items))
}

#[derive(Deserialize)]
struct IgnoreBody {
    ignored: bool,
}

async fn set_session_ignore(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<IgnoreBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    state
        .storage
        .set_session_ignored(&id, body.ignored)
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(json!({ "ok": true, "session_id": id, "ignored": body.ignored })))
}

#[derive(Deserialize, Default)]
struct StateAtQuery {
    at_seq: Option<i64>,
}

async fn get_session_state(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(q): Query<StateAtQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let store = state
        .storage
        .reconstruct_session_state(&id, q.at_seq)
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(store.snapshot_json(Some(&id))))
}

#[derive(Deserialize)]
struct SearchQuery {
    q: String,
    session_id: Option<String>,
    exclude_session_id: Option<String>,
    kind: Option<String>,
    limit: Option<usize>,
}

async fn search_traffic(
    State(state): State<Arc<AppState>>,
    Query(q): Query<SearchQuery>,
) -> Result<Json<Vec<SearchHit>>, (StatusCode, Json<Value>)> {
    let hits = state
        .storage
        .search_traffic(
            &q.q,
            q.session_id.as_deref(),
            q.kind.as_deref(),
            q.exclude_session_id.as_deref(),
            q.limit.unwrap_or(50),
        )
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(hits))
}

#[derive(Deserialize, Default)]
struct AnomaliesQuery {
    session_id: Option<String>,
    exclude_session_id: Option<String>,
    slow_ms: Option<i64>,
    limit: Option<usize>,
}

async fn get_anomalies(
    State(state): State<Arc<AppState>>,
    Query(q): Query<AnomaliesQuery>,
) -> Result<Json<Vec<Anomaly>>, (StatusCode, Json<Value>)> {
    let opts = AnomalyOptions {
        session_id: q.session_id,
        exclude_session_id: q.exclude_session_id,
        slow_http_ms: q.slow_ms.unwrap_or(3000),
        limit: q.limit.unwrap_or(100),
    };
    let list =
        find_anomalies(&state.storage, &opts).map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(list))
}

// ---- Mock Rules ----

async fn list_rules(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<MockRule>>, (StatusCode, Json<Value>)> {
    let rules = state
        .storage
        .list_mock_rules()
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(rules))
}

async fn create_rule(
    State(state): State<Arc<AppState>>,
    Json(input): Json<CreateMockRuleInput>,
) -> Result<Json<MockRule>, (StatusCode, Json<Value>)> {
    let rule = MockRule {
        id: format!("rule_{}", &uuid::Uuid::new_v4().simple().to_string()[..10]),
        name: input.name,
        enabled: input.enabled,
        priority: input.priority,
        method: input.method.to_uppercase(),
        path_pattern: input.path_pattern,
        directory_filter: input.directory_filter,
        action_type: input.action_type,
        status: input.status,
        delay_ms: input.delay_ms,
        headers: input.headers,
        body: input.body,
        patch: input.patch,
        created_at: now_ms(),
    };
    state
        .storage
        .upsert_mock_rule(&rule)
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    state.broadcast(ControlWsEvent::RulesUpdated);
    Ok(Json(rule))
}

async fn update_rule(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(input): Json<CreateMockRuleInput>,
) -> Result<Json<MockRule>, (StatusCode, Json<Value>)> {
    let rule = MockRule {
        id,
        name: input.name,
        enabled: input.enabled,
        priority: input.priority,
        method: input.method.to_uppercase(),
        path_pattern: input.path_pattern,
        directory_filter: input.directory_filter,
        action_type: input.action_type,
        status: input.status,
        delay_ms: input.delay_ms,
        headers: input.headers,
        body: input.body,
        patch: input.patch,
        created_at: now_ms(),
    };
    state
        .storage
        .upsert_mock_rule(&rule)
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    state.broadcast(ControlWsEvent::RulesUpdated);
    Ok(Json(rule))
}

async fn delete_rule(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let deleted = state
        .storage
        .delete_mock_rule(&id)
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    state.broadcast(ControlWsEvent::RulesUpdated);
    Ok(Json(json!({ "deleted": deleted })))
}

// ---- Breakpoints ----

async fn list_breakpoints(State(state): State<Arc<AppState>>) -> Json<Vec<PendingBreakpoint>> {
    let mut items: Vec<PendingBreakpoint> = state
        .breakpoints
        .iter()
        .map(|e| e.value().info.clone())
        .collect();
    items.sort_by_key(|b| b.started_at);
    Json(items)
}

async fn resolve_breakpoint(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(decision): Json<BreakpointDecision>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let (_, entry) = state
        .breakpoints
        .remove(&id)
        .ok_or_else(|| api_err(StatusCode::NOT_FOUND, "breakpoint not found or already resolved"))?;
    let _ = entry.tx.send(decision);
    state.broadcast(ControlWsEvent::BreakpointResolved { id: id.clone() });
    state.broadcast_status();
    Ok(Json(json!({ "resolved": true, "id": id })))
}

// ---- Tapes & Replay ----

async fn list_tapes(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<TapeSummary>>, (StatusCode, Json<Value>)> {
    let list = state
        .storage
        .list_tapes()
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(list))
}

async fn get_tape(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Tape>, (StatusCode, Json<Value>)> {
    let tape = state
        .storage
        .get_tape(&id)
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e))?
        .ok_or_else(|| api_err(StatusCode::NOT_FOUND, "tape not found"))?;
    Ok(Json(tape))
}

async fn save_tape(
    State(state): State<Arc<AppState>>,
    Json(mut tape): Json<Tape>,
) -> Result<Json<Tape>, (StatusCode, Json<Value>)> {
    if tape.id.is_empty() {
        tape.id = format!("tape_{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
    }
    if tape.created_at == 0 {
        tape.created_at = now_ms();
    }
    state
        .storage
        .save_tape(&tape)
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    state.broadcast(ControlWsEvent::TapesUpdated);
    Ok(Json(tape))
}

#[derive(Deserialize, Default)]
struct ExtractTapeBody {
    name: Option<String>,
}

async fn extract_tape(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    body: Option<Json<ExtractTapeBody>>,
) -> Result<Json<Tape>, (StatusCode, Json<Value>)> {
    let name = body.and_then(|b| b.0.name);
    let tape = state
        .storage
        .extract_tape_from_session(&session_id, name)
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    state.broadcast(ControlWsEvent::TapesUpdated);
    Ok(Json(tape))
}

#[derive(Deserialize)]
struct ImportTapeBody {
    jsonl: String,
}

async fn import_tape(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ImportTapeBody>,
) -> Result<Json<Tape>, (StatusCode, Json<Value>)> {
    let tape = import_tape_jsonl(&body.jsonl).map_err(|e| api_err(StatusCode::BAD_REQUEST, e))?;
    state
        .storage
        .save_tape(&tape)
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    state.broadcast(ControlWsEvent::TapesUpdated);
    Ok(Json(tape))
}

async fn export_tape(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Response, (StatusCode, Json<Value>)> {
    let tape = state
        .storage
        .get_tape(&id)
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e))?
        .ok_or_else(|| api_err(StatusCode::NOT_FOUND, "tape not found"))?;
    let jsonl = export_tape_jsonl(&tape).map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/x-ndjson; charset=utf-8")],
        jsonl,
    )
        .into_response())
}

async fn delete_tape(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let deleted = state
        .storage
        .delete_tape(&id)
        .map_err(|e| api_err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    state.broadcast(ControlWsEvent::TapesUpdated);
    Ok(Json(json!({ "deleted": deleted })))
}

async fn get_replay_status(State(state): State<Arc<AppState>>) -> Json<ReplayStatus> {
    Json(state.replay.lock().unwrap().status.clone())
}

#[derive(Deserialize)]
struct ReplayLoadBody {
    tape_id: String,
    target_directory: Option<String>,
    #[serde(default = "default_true_flag")]
    mount_shadow: bool,
    speed: Option<f64>,
    min_frame_ms: Option<u64>,
}
fn default_true_flag() -> bool {
    true
}

async fn replay_load(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ReplayLoadBody>,
) -> Result<Json<ReplayStatus>, (StatusCode, Json<Value>)> {
    let st = replay::load_tape(
        &state,
        &body.tape_id,
        body.target_directory,
        body.mount_shadow,
        body.speed,
        body.min_frame_ms,
    )
    .map_err(|e| api_err(StatusCode::BAD_REQUEST, e))?;
    Ok(Json(st))
}

async fn replay_play(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ReplayStatus>, (StatusCode, Json<Value>)> {
    let st = replay::play(&state).map_err(|e| api_err(StatusCode::BAD_REQUEST, e))?;
    Ok(Json(st))
}

async fn replay_pause(State(state): State<Arc<AppState>>) -> Json<ReplayStatus> {
    Json(replay::pause(&state))
}

async fn replay_stop(State(state): State<Arc<AppState>>) -> Json<ReplayStatus> {
    Json(replay::stop(&state))
}

async fn replay_step(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ReplayStatus>, (StatusCode, Json<Value>)> {
    let st = replay::step_next(&state).map_err(|e| api_err(StatusCode::BAD_REQUEST, e))?;
    Ok(Json(st))
}

#[derive(Deserialize)]
struct ReplaySeekBody {
    cursor: usize,
    #[serde(default = "default_true_flag")]
    notify_clients: bool,
}

async fn replay_seek(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ReplaySeekBody>,
) -> Result<Json<ReplayStatus>, (StatusCode, Json<Value>)> {
    let st = replay::seek(&state, body.cursor, body.notify_clients)
        .map_err(|e| api_err(StatusCode::BAD_REQUEST, e))?;
    Ok(Json(st))
}

#[derive(Deserialize)]
struct ReplaySpeedBody {
    speed: f64,
    min_frame_ms: Option<u64>,
}

async fn replay_speed(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ReplaySpeedBody>,
) -> Json<ReplayStatus> {
    Json(replay::set_speed(&state, body.speed, body.min_frame_ms))
}

#[derive(Deserialize, Default)]
struct TuiSelectBody {
    session_id: Option<String>,
}

async fn replay_tui_select(
    State(state): State<Arc<AppState>>,
    body: Option<Json<TuiSelectBody>>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let sid = body.and_then(|b| b.0.session_id);
    let payload = replay::select_in_tui(&state, sid)
        .map_err(|e| api_err(StatusCode::BAD_REQUEST, e))?;
    Ok(Json(json!({ "ok": true, "event": payload })))
}

// ---- OpenAPI /doc & Faker ----

async fn get_openapi(State(state): State<Arc<AppState>>) -> Json<OpenApiSummary> {
    Json(state.openapi.read().unwrap().summary())
}

#[derive(Deserialize, Default)]
struct SyncOpenApiBody {
    url: Option<String>,
}

async fn sync_openapi(
    State(state): State<Arc<AppState>>,
    body: Option<Json<SyncOpenApiBody>>,
) -> Result<Json<OpenApiSummary>, (StatusCode, Json<Value>)> {
    let target = body
        .and_then(|b| b.0.url)
        .or_else(|| state.config.upstream_url.clone())
        .ok_or_else(|| api_err(StatusCode::BAD_REQUEST, "no upstream URL configured"))?;
    let reg = OpenApiRegistry::fetch_from_upstream(&target)
        .await
        .map_err(|e| api_err(StatusCode::BAD_GATEWAY, e))?;
    let summary = reg.summary();
    *state.openapi.write().unwrap() = reg;
    state.broadcast_status();
    Ok(Json(summary))
}

#[derive(Deserialize)]
struct FakeHttpBody {
    method: String,
    path: String,
    directory: Option<String>,
}

async fn openapi_fake_http(
    State(state): State<Arc<AppState>>,
    Json(body): Json<FakeHttpBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let dir = body
        .directory
        .unwrap_or_else(|| state.primary_active_directory());
    let val = state
        .openapi
        .read()
        .unwrap()
        .fake_http_response(&body.method, &body.path, &dir)
        .map_err(|e| api_err(StatusCode::BAD_REQUEST, e))?;
    Ok(Json(val))
}

#[derive(Deserialize)]
struct FakeSseBody {
    event_type: String,
    directory: Option<String>,
}

async fn openapi_fake_sse(
    State(state): State<Arc<AppState>>,
    Json(body): Json<FakeSseBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let dir = body
        .directory
        .unwrap_or_else(|| state.primary_active_directory());
    let val = state
        .openapi
        .read()
        .unwrap()
        .fake_sse_event(&body.event_type, &dir)
        .map_err(|e| api_err(StatusCode::BAD_REQUEST, e))?;
    Ok(Json(val))
}

// ---- LLM Generator & Mutator ----

async fn get_llm_config(State(state): State<Arc<AppState>>) -> Json<Value> {
    let cfg = state.llm_config.read().unwrap().clone();
    match cfg {
        Some(c) => Json(json!({
            "configured": true,
            "api_base": c.api_base,
            "model": c.model,
            "temperature": c.temperature,
            "has_api_key": !c.api_key.is_empty(),
        })),
        None => Json(json!({ "configured": false })),
    }
}

async fn set_llm_config(
    State(state): State<Arc<AppState>>,
    Json(cfg): Json<LlmConfig>,
) -> Json<Value> {
    *state.llm_config.write().unwrap() = Some(cfg);
    Json(json!({ "ok": true }))
}

#[derive(Deserialize)]
struct LlmHttpBody {
    method: String,
    path: String,
    prompt: String,
    directory: Option<String>,
    llm: Option<LlmConfig>,
}

async fn llm_generate_http(
    State(state): State<Arc<AppState>>,
    Json(body): Json<LlmHttpBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let cfg = body
        .llm
        .or_else(|| state.llm_config.read().unwrap().clone())
        .ok_or_else(|| {
            api_err(
                StatusCode::BAD_REQUEST,
                "LLM config not set (configure via Settings or OC_LLM_API_BASE + OC_LLM_MODEL)",
            )
        })?;
    let dir = body
        .directory
        .unwrap_or_else(|| state.primary_active_directory());
    let reg = state.openapi.read().unwrap().clone();
    let res = generate_http_mock(&cfg, &reg, &body.method, &body.path, &body.prompt, &dir)
        .await
        .map_err(|e| api_err(StatusCode::BAD_GATEWAY, e))?;
    Ok(Json(serde_json::to_value(res).unwrap_or(Value::Null)))
}

#[derive(Deserialize)]
struct LlmScenarioBody {
    prompt: String,
    directory: Option<String>,
    tape_name: Option<String>,
    #[serde(default = "default_true_flag")]
    save_tape: bool,
    llm: Option<LlmConfig>,
}

async fn llm_generate_scenario(
    State(state): State<Arc<AppState>>,
    Json(body): Json<LlmScenarioBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let cfg = body
        .llm
        .or_else(|| state.llm_config.read().unwrap().clone())
        .ok_or_else(|| {
            api_err(
                StatusCode::BAD_REQUEST,
                "LLM config not set (configure via Settings or OC_LLM_API_BASE + OC_LLM_MODEL)",
            )
        })?;
    let dir = body
        .directory
        .unwrap_or_else(|| state.primary_active_directory());
    let reg = state.openapi.read().unwrap().clone();
    let res = generate_sse_scenario(&cfg, &reg, &body.prompt, &dir, body.tape_name)
        .await
        .map_err(|e| api_err(StatusCode::BAD_GATEWAY, e))?;
    if body.save_tape {
        let _ = state.storage.save_tape(&res.tape);
        state.broadcast(ControlWsEvent::TapesUpdated);
    }
    Ok(Json(serde_json::to_value(res).unwrap_or(Value::Null)))
}

#[derive(Deserialize)]
struct LlmMutateBody {
    payload: Value,
    instruction: String,
    llm: Option<LlmConfig>,
}

async fn llm_mutate(
    State(state): State<Arc<AppState>>,
    Json(body): Json<LlmMutateBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let cfg = body
        .llm
        .or_else(|| state.llm_config.read().unwrap().clone())
        .ok_or_else(|| api_err(StatusCode::BAD_REQUEST, "LLM config not set"))?;
    let mutated = mutate_payload(&cfg, &body.payload, &body.instruction)
        .await
        .map_err(|e| api_err(StatusCode::BAD_GATEWAY, e))?;
    Ok(Json(json!({ "data": mutated })))
}

// ---- Live DevTools WebSocket ----

async fn handle_ws(State(state): State<Arc<AppState>>, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| drive_control_ws(state, socket))
}

async fn drive_control_ws(state: Arc<AppState>, mut socket: WebSocket) {
    let init = ControlWsEvent::Status {
        status: state.runtime_status(),
    };
    if let Ok(text) = serde_json::to_string(&init) {
        let _ = socket.send(Message::Text(text.into())).await;
    }

    let mut rx = state.ws_tx.subscribe();
    loop {
        tokio::select! {
            ev = rx.recv() => {
                match ev {
                    Ok(item) => {
                        if let Ok(text) = serde_json::to_string(&item) {
                            if socket.send(Message::Text(text.into())).await.is_err() {
                                break;
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                }
            }
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
        }
    }
}

// ---- Embedded Web DevTools SPA ----

async fn serve_web_ui(uri: Uri) -> Response {
    let raw_path = uri.path().trim_start_matches('/');
    let path = if raw_path.is_empty() {
        "index.html"
    } else {
        raw_path
    };

    if let Some(file) = WebAssets::get(path) {
        let mime = mime_guess::from_path(path).first_or_octet_stream();
        return (
            StatusCode::OK,
            [(header::CONTENT_TYPE, mime.as_ref())],
            file.data,
        )
            .into_response();
    }

    if let Some(index) = WebAssets::get("index.html") {
        return (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            index.data,
        )
            .into_response();
    }

    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        "<h1>oc-mock-server Control Plane</h1><p>Build web/dist to embed the DevTools UI.</p>",
    )
        .into_response()
}
