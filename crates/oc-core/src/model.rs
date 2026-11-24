use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyMode {
    Passthrough,
    Intercept,
    Replay,
}

impl Default for ProxyMode {
    fn default() -> Self {
        Self::Passthrough
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpTrace {
    pub id: String,
    pub started_at: i64,
    pub completed_at: Option<i64>,
    pub duration_ms: Option<i64>,
    pub method: String,
    pub path: String,
    pub query: Option<String>,
    pub directory: Option<String>,
    pub session_id: Option<String>,
    pub status: Option<u16>,
    pub req_headers: BTreeMap<String, String>,
    pub req_body: Option<String>,
    pub req_size: usize,
    pub res_headers: BTreeMap<String, String>,
    pub res_body: Option<String>,
    pub res_size: usize,
    pub mocked: bool,
    pub mock_rule_id: Option<String>,
    pub upstream_error: Option<String>,
    pub pending: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpTraceSummary {
    pub id: String,
    pub started_at: i64,
    pub duration_ms: Option<i64>,
    pub method: String,
    pub path: String,
    pub query: Option<String>,
    pub directory: Option<String>,
    pub session_id: Option<String>,
    pub status: Option<u16>,
    pub req_size: usize,
    pub res_size: usize,
    pub mocked: bool,
    pub mock_rule_id: Option<String>,
    pub upstream_error: Option<String>,
    pub pending: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SseSource {
    Upstream,
    Injected,
    Replay,
}

impl SseSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Upstream => "upstream",
            Self::Injected => "injected",
            Self::Replay => "replay",
        }
    }

    pub fn parse(raw: &str) -> Self {
        match raw {
            "injected" => Self::Injected,
            "replay" => Self::Replay,
            _ => Self::Upstream,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SseRecord {
    pub seq: i64,
    pub ts: i64,
    pub source: SseSource,
    pub directory: String,
    pub event_type: String,
    pub session_id: Option<String>,
    pub message_id: Option<String>,
    pub part_id: Option<String>,
    pub payload: Value,
    #[serde(default)]
    pub mocked: bool,
    #[serde(default)]
    pub mock_rule_id: Option<String>,
    #[serde(default)]
    pub dropped: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionOverview {
    pub session_id: String,
    pub directory: Option<String>,
    pub title: Option<String>,
    pub first_ts: i64,
    pub last_ts: i64,
    pub http_count: usize,
    pub http_error_count: usize,
    pub sse_count: usize,
    pub ignored: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TimelineItem {
    Http {
        ts: i64,
        trace: HttpTraceSummary,
    },
    Sse {
        ts: i64,
        event: SseRecord,
    },
    FoldedDelta {
        ts: i64,
        end_ts: i64,
        from_seq: i64,
        to_seq: i64,
        count: usize,
        directory: String,
        session_id: Option<String>,
        message_id: String,
        part_id: String,
        field: String,
        total_chars: usize,
        duration_ms: i64,
        preview: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SearchHit {
    Http {
        id: String,
        ts: i64,
        method: String,
        path: String,
        status: Option<u16>,
        session_id: Option<String>,
        snippet: String,
    },
    Sse {
        seq: i64,
        ts: i64,
        event_type: String,
        session_id: Option<String>,
        directory: String,
        snippet: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnomalySeverity {
    Error,
    Warning,
    Info,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Anomaly {
    pub severity: AnomalySeverity,
    pub kind: String,
    pub ts: i64,
    pub session_id: Option<String>,
    pub ref_id: String,
    pub message: String,
    pub details: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MockActionType {
    Direct,
    Patch,
    Breakpoint,
    SseStream,
}

impl MockActionType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Patch => "patch",
            Self::Breakpoint => "breakpoint",
            Self::SseStream => "sse_stream",
        }
    }

    pub fn parse(raw: &str) -> Self {
        match raw {
            "patch" => Self::Patch,
            "breakpoint" => Self::Breakpoint,
            "sse_stream" => Self::SseStream,
            _ => Self::Direct,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BreakpointPhase {
    #[default]
    Request,
    Response,
    Both,
}

impl BreakpointPhase {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Request => "request",
            Self::Response => "response",
            Self::Both => "both",
        }
    }

    pub fn parse(raw: &str) -> Self {
        match raw {
            "response" => Self::Response,
            "both" => Self::Both,
            _ => Self::Request,
        }
    }

    pub fn intercepts_request(&self) -> bool {
        matches!(self, Self::Request | Self::Both)
    }

    pub fn intercepts_response(&self) -> bool {
        matches!(self, Self::Response | Self::Both)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MockSseStep {
    #[serde(default)]
    pub delay_ms: u64,
    pub payload: Value,
    #[serde(default)]
    pub directory: Option<String>,
    #[serde(default)]
    pub event: Option<String>,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default = "default_true")]
    pub apply_to_shadow: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MockRule {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub priority: i32,
    pub method: String,
    pub path_pattern: String,
    #[serde(default)]
    pub query_pattern: Option<String>,
    #[serde(default)]
    pub body_contains: Option<String>,
    pub directory_filter: Option<String>,
    pub action_type: MockActionType,
    #[serde(default)]
    pub breakpoint_phase: BreakpointPhase,
    #[serde(default)]
    pub one_shot: bool,
    #[serde(default)]
    pub hit_count: u64,
    pub status: u16,
    pub delay_ms: u64,
    pub headers: BTreeMap<String, String>,
    pub body: Option<Value>,
    pub patch: Option<Value>,
    #[serde(default)]
    pub sse_sequence: Vec<MockSseStep>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateMockRuleInput {
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_priority")]
    pub priority: i32,
    #[serde(default = "default_method")]
    pub method: String,
    pub path_pattern: String,
    #[serde(default)]
    pub query_pattern: Option<String>,
    #[serde(default)]
    pub body_contains: Option<String>,
    pub directory_filter: Option<String>,
    #[serde(default = "default_action")]
    pub action_type: MockActionType,
    #[serde(default)]
    pub breakpoint_phase: BreakpointPhase,
    #[serde(default)]
    pub one_shot: bool,
    #[serde(default = "default_status")]
    pub status: u16,
    #[serde(default)]
    pub delay_ms: u64,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    pub body: Option<Value>,
    pub patch: Option<Value>,
    #[serde(default)]
    pub sse_sequence: Vec<MockSseStep>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SseMockActionType {
    Drop,
    #[default]
    Patch,
    Replace,
    Delay,
    Breakpoint,
}

impl SseMockActionType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Drop => "drop",
            Self::Patch => "patch",
            Self::Replace => "replace",
            Self::Delay => "delay",
            Self::Breakpoint => "breakpoint",
        }
    }

    pub fn parse(raw: &str) -> Self {
        match raw {
            "drop" => Self::Drop,
            "replace" => Self::Replace,
            "delay" => Self::Delay,
            "breakpoint" => Self::Breakpoint,
            _ => Self::Patch,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SseMockRule {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub priority: i32,
    pub event_type_pattern: String,
    #[serde(default)]
    pub session_filter: Option<String>,
    #[serde(default)]
    pub directory_filter: Option<String>,
    #[serde(default)]
    pub payload_contains: Option<String>,
    pub action_type: SseMockActionType,
    #[serde(default)]
    pub delay_ms: u64,
    #[serde(default)]
    pub patch: Option<Value>,
    #[serde(default)]
    pub replace_events: Vec<MockSseStep>,
    #[serde(default)]
    pub one_shot: bool,
    #[serde(default)]
    pub hit_count: u64,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSseMockRuleInput {
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_priority")]
    pub priority: i32,
    #[serde(default = "default_wildcard")]
    pub event_type_pattern: String,
    #[serde(default)]
    pub session_filter: Option<String>,
    #[serde(default)]
    pub directory_filter: Option<String>,
    #[serde(default)]
    pub payload_contains: Option<String>,
    #[serde(default)]
    pub action_type: SseMockActionType,
    #[serde(default)]
    pub delay_ms: u64,
    #[serde(default)]
    pub patch: Option<Value>,
    #[serde(default)]
    pub replace_events: Vec<MockSseStep>,
    #[serde(default)]
    pub one_shot: bool,
}

fn default_wildcard() -> String {
    "*".to_string()
}

fn default_true() -> bool {
    true
}
fn default_priority() -> i32 {
    100
}
fn default_method() -> String {
    "ANY".to_string()
}
fn default_action() -> MockActionType {
    MockActionType::Direct
}
fn default_status() -> u16 {
    200
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BreakpointKind {
    #[default]
    HttpRequest,
    HttpResponse,
    SseEvent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingBreakpoint {
    pub id: String,
    #[serde(default)]
    pub kind: BreakpointKind,
    pub trace_id: String,
    pub rule_id: String,
    pub rule_name: String,
    pub started_at: i64,
    pub method: String,
    pub path: String,
    pub query: Option<String>,
    pub directory: Option<String>,
    pub req_headers: BTreeMap<String, String>,
    pub req_body: Option<String>,
    #[serde(default)]
    pub res_status: Option<u16>,
    #[serde(default)]
    pub res_headers: BTreeMap<String, String>,
    #[serde(default)]
    pub res_body: Option<String>,
    #[serde(default)]
    pub sse_event_type: Option<String>,
    #[serde(default)]
    pub sse_session_id: Option<String>,
    #[serde(default)]
    pub sse_payload: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum BreakpointDecision {
    Forward {
        #[serde(default)]
        method: Option<String>,
        #[serde(default)]
        path: Option<String>,
        #[serde(default)]
        query: Option<String>,
        #[serde(default)]
        req_body: Option<String>,
        #[serde(default)]
        req_headers: Option<BTreeMap<String, String>>,
    },
    Respond {
        status: u16,
        #[serde(default)]
        headers: BTreeMap<String, String>,
        body: String,
    },
    EmitSse {
        payload: Value,
        #[serde(default)]
        directory: Option<String>,
        #[serde(default)]
        apply_to_shadow: Option<bool>,
    },
    Drop,
    Abort {
        message: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TapeSnapshot {
    #[serde(default)]
    pub projects: Vec<Value>,
    #[serde(default)]
    pub sessions: Vec<Value>,
    #[serde(default)]
    pub messages: BTreeMap<String, Vec<Value>>,
    #[serde(default)]
    pub parts: BTreeMap<String, Vec<Value>>,
    #[serde(default)]
    pub statuses: BTreeMap<String, Value>,
    #[serde(default)]
    pub todos: BTreeMap<String, Vec<Value>>,
    #[serde(default)]
    pub diffs: BTreeMap<String, Vec<Value>>,
}

impl Default for TapeSnapshot {
    fn default() -> Self {
        Self {
            projects: Vec::new(),
            sessions: Vec::new(),
            messages: BTreeMap::new(),
            parts: BTreeMap::new(),
            statuses: BTreeMap::new(),
            todos: BTreeMap::new(),
            diffs: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TapeEvent {
    pub offset_ms: u64,
    pub directory: String,
    pub payload: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tape {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub directory: String,
    pub session_id: Option<String>,
    pub snapshot: TapeSnapshot,
    pub events: Vec<TapeEvent>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TapeSummary {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub directory: String,
    pub session_id: Option<String>,
    pub event_count: usize,
    pub duration_ms: u64,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayStateKind {
    Stopped,
    Playing,
    Paused,
    Completed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayStatus {
    pub state: ReplayStateKind,
    pub tape_id: Option<String>,
    pub tape_name: Option<String>,
    pub session_id: Option<String>,
    pub cursor: usize,
    pub total_events: usize,
    pub speed: f64,
    pub min_frame_ms: u64,
    pub target_directory: Option<String>,
    pub mount_shadow: bool,
    pub web_session_url: Option<String>,
}

impl Default for ReplayStatus {
    fn default() -> Self {
        Self {
            state: ReplayStateKind::Stopped,
            tape_id: None,
            tape_name: None,
            session_id: None,
            cursor: 0,
            total_events: 0,
            speed: 1.0,
            min_frame_ms: 20,
            target_directory: None,
            mount_shadow: true,
            web_session_url: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenApiOperation {
    pub method: String,
    pub path: String,
    pub operation_id: Option<String>,
    pub summary: Option<String>,
    pub description: Option<String>,
    pub response_schema: Option<Value>,
    pub request_schema: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenApiEventSchema {
    pub event_type: String,
    pub schema: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenApiSummary {
    pub source: String,
    pub title: String,
    pub version: String,
    pub operations: Vec<OpenApiOperation>,
    pub events: Vec<OpenApiEventSchema>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    pub api_base: String,
    pub api_key: String,
    pub model: String,
    #[serde(default = "default_temp")]
    pub temperature: f32,
}

fn default_temp() -> f32 {
    0.3
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyRuntimeStatus {
    pub proxy_addr: String,
    pub control_addr: String,
    pub upstream_url: Option<String>,
    pub mode: ProxyMode,
    pub mute_upstream_sse: bool,
    pub recording: bool,
    pub active_directories: Vec<String>,
    pub connected_sse_clients: usize,
    pub pending_breakpoints: usize,
    pub openapi_source: String,
    pub replay: ReplayStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlWsEvent {
    Status { status: ProxyRuntimeStatus },
    HttpStarted { trace: HttpTraceSummary },
    HttpCompleted { trace: HttpTraceSummary },
    SseRecorded { event: SseRecord },
    BreakpointAdded { breakpoint: PendingBreakpoint },
    BreakpointResolved { id: String },
    ReplayUpdated { replay: ReplayStatus },
    RulesUpdated,
    SseRulesUpdated,
    TapesUpdated,
}

/// Extract semantic IDs (`session_id`, `message_id`, `part_id`) from an OpenCode SSE event payload.
pub fn extract_event_ids(payload: &Value) -> (Option<String>, Option<String>, Option<String>) {
    let props = match payload.get("properties") {
        Some(v) => v,
        None => return (None, None, None),
    };

    let event_type = payload.get("type").and_then(Value::as_str).unwrap_or("");
    match event_type {
        "session.created" | "session.updated" | "session.deleted" => {
            let sid = props
                .get("info")
                .and_then(|v| v.get("id"))
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);
            (sid, None, None)
        }
        "message.updated" => {
            let info = props.get("info");
            let sid = info
                .and_then(|v| v.get("sessionID"))
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);
            let mid = info
                .and_then(|v| v.get("id"))
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);
            (sid, mid, None)
        }
        "message.removed" => {
            let sid = props.get("sessionID").and_then(Value::as_str).map(ToOwned::to_owned);
            let mid = props.get("messageID").and_then(Value::as_str).map(ToOwned::to_owned);
            (sid, mid, None)
        }
        "message.part.updated" => {
            let part = props.get("part");
            let sid = part
                .and_then(|v| v.get("sessionID"))
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);
            let mid = part
                .and_then(|v| v.get("messageID"))
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);
            let pid = part
                .and_then(|v| v.get("id"))
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);
            (sid, mid, pid)
        }
        "message.part.delta" | "message.part.removed" => {
            let sid = props.get("sessionID").and_then(Value::as_str).map(ToOwned::to_owned);
            let mid = props.get("messageID").and_then(Value::as_str).map(ToOwned::to_owned);
            let pid = props.get("partID").and_then(Value::as_str).map(ToOwned::to_owned);
            (sid, mid, pid)
        }
        _ => {
            let sid = props
                .get("sessionID")
                .and_then(Value::as_str)
                .or_else(|| {
                    props
                        .get("info")
                        .and_then(|v| v.get("sessionID"))
                        .and_then(Value::as_str)
                })
                .map(ToOwned::to_owned);
            let mid = props.get("messageID").and_then(Value::as_str).map(ToOwned::to_owned);
            let pid = props.get("partID").and_then(Value::as_str).map(ToOwned::to_owned);
            (sid, mid, pid)
        }
    }
}

/// Extract session ID from an OpenCode HTTP path like `/session/ses_xxx/message`.
pub fn extract_session_from_path(path: &str) -> Option<String> {
    let trimmed = path.trim_start_matches('/');
    let mut parts = trimmed.split('/');
    let first = parts.next()?;
    if first != "session" {
        return None;
    }
    let second = parts.next()?;
    if second.is_empty() || second == "status" {
        return None;
    }
    Some(second.to_string())
}

/// Extract session ID from a JSON response body (e.g., `POST /session` returning `{ "id": "ses_..." }`).
pub fn extract_session_from_body(path: &str, body: &str) -> Option<String> {
    if path.trim_end_matches('/') != "/session" {
        return None;
    }
    let val: Value = serde_json::from_str(body).ok()?;
    val.get("id").and_then(Value::as_str).map(ToOwned::to_owned)
}

/// Glob wildcard matcher supporting `*` (0+ chars) and `?` (1 char).
pub fn wildcard_matches(pattern: &str, target: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = target.chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star_pi, mut star_ti) = (None::<usize>, 0usize);

    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star_pi = Some(pi);
            star_ti = ti;
            pi += 1;
        } else if let Some(sp) = star_pi {
            pi = sp + 1;
            star_ti += 1;
            ti = star_ti;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// Match an identifier (such as `event_type`, `session_id`, `directory`) against exact text,
/// `*`/`?` wildcards, or `regex:<pattern>`.
pub fn pattern_matches(pattern: &str, target: &str) -> bool {
    let trimmed = pattern.trim();
    if trimmed.is_empty() || trimmed == "*" {
        return true;
    }
    if let Some(re_src) = trimmed.strip_prefix("regex:") {
        return fancy_regex::Regex::new(re_src)
            .ok()
            .and_then(|re| re.is_match(target).ok())
            .unwrap_or(false);
    }
    if trimmed.contains('*') || trimmed.contains('?') {
        return wildcard_matches(trimmed, target);
    }
    trimmed == target
}

/// Match substring, `*` glob, or `regex:<pattern>` against query strings or JSON/text bodies.
pub fn substring_or_regex_matches(pattern: &str, target: &str) -> bool {
    let trimmed = pattern.trim();
    if trimmed.is_empty() || trimmed == "*" {
        return true;
    }
    if let Some(re_src) = trimmed.strip_prefix("regex:") {
        return fancy_regex::Regex::new(re_src)
            .ok()
            .and_then(|re| re.is_match(target).ok())
            .unwrap_or(false);
    }
    if trimmed.contains('*') {
        return wildcard_matches(trimmed, target);
    }
    target.contains(trimmed)
}

/// Match a URL path against a pattern supporting `:param` segments, `*` wildcards, and `regex:<expr>`.
pub fn path_matches(pattern: &str, path: &str) -> bool {
    let trimmed = pattern.trim();
    if let Some(re_src) = trimmed.strip_prefix("regex:") {
        return fancy_regex::Regex::new(re_src)
            .ok()
            .and_then(|re| re.is_match(path).ok())
            .unwrap_or(false);
    }

    let pat = trimmed.trim_end_matches('/');
    let tgt = path.trim_end_matches('/');
    if pat.is_empty() && tgt.is_empty() {
        return true;
    }
    if pat == "*" || pat == "/*" {
        return true;
    }
    if let Some(prefix) = pat.strip_suffix("/*") {
        if !prefix.contains(':') && !prefix.contains('*') {
            return tgt == prefix || tgt.starts_with(&format!("{prefix}/"));
        }
    }
    if let Some(prefix) = pat.strip_suffix('*') {
        if !prefix.contains(':') && !prefix.contains('*') {
            return tgt.starts_with(prefix);
        }
    }

    let pat_segs: Vec<&str> = pat.split('/').collect();
    let tgt_segs: Vec<&str> = tgt.split('/').collect();
    if pat_segs.len() == tgt_segs.len()
        && pat_segs
            .iter()
            .zip(tgt_segs.iter())
            .all(|(p, t)| p.starts_with(':') || *p == "*" || wildcard_matches(p, t))
    {
        return true;
    }

    if pat.contains('*') && !pat.contains(':') {
        return wildcard_matches(pat, tgt);
    }
    false
}

/// RFC 7396 JSON Merge Patch shared by HTTP Patch rules and SSE Patch rules.
pub fn json_merge_patch(target: &mut Value, patch: &Value) {
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

#[derive(Debug, Clone, Default)]
pub struct TemplateContext {
    pub session_id: String,
    pub directory: String,
    pub method: String,
    pub path: String,
}

impl TemplateContext {
    pub fn apply_str(&self, input: &str) -> String {
        if !input.contains("{{") {
            return input.to_string();
        }
        let ts_str = now_ms().to_string();
        input
            .replace("{{session_id}}", &self.session_id)
            .replace("{{sessionID}}", &self.session_id)
            .replace("{{directory}}", &self.directory)
            .replace("{{method}}", &self.method)
            .replace("{{path}}", &self.path)
            .replace("{{timestamp}}", &ts_str)
    }

    pub fn apply_value(&self, val: &Value) -> Value {
        match val {
            Value::String(s) => {
                if s == "{{timestamp_ms}}" {
                    Value::Number(now_ms().into())
                } else {
                    Value::String(self.apply_str(s))
                }
            }
            Value::Array(arr) => Value::Array(arr.iter().map(|v| self.apply_value(v)).collect()),
            Value::Object(map) => {
                let mut out = serde_json::Map::with_capacity(map.len());
                for (k, v) in map {
                    out.insert(self.apply_str(k), self.apply_value(v));
                }
                Value::Object(out)
            }
            other => other.clone(),
        }
    }
}

/// Encode a directory path the same way OpenCode Web App encodes URL route segments.
pub fn encode_opencode_dir(dir: &str) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(dir.as_bytes())
}

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
