use crate::model::*;
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// Rust mirror of OpenCode's frontend `event-reducer.ts` store.
/// Tracks virtual Session / Message / Part / Status / Todo / Diff / Permission / Question state
/// for both Replay virtual HTTP serving and MCP `reconstruct_state_at`.
#[derive(Debug, Clone, Default)]
pub struct ShadowStore {
    pub directory: String,
    pub projects: BTreeMap<String, Value>,
    pub sessions: BTreeMap<String, Value>,
    /// session_id -> (message_id -> Message)
    pub messages: BTreeMap<String, BTreeMap<String, Value>>,
    /// message_id -> (part_id -> Part)
    pub parts: BTreeMap<String, BTreeMap<String, Value>>,
    /// session_id -> SessionStatus
    pub statuses: BTreeMap<String, Value>,
    /// session_id -> Vec<Todo>
    pub todos: BTreeMap<String, Vec<Value>>,
    /// session_id -> Vec<FileDiff>
    pub diffs: BTreeMap<String, Vec<Value>>,
    /// session_id -> (permission_id -> PermissionRequest)
    pub permissions: BTreeMap<String, BTreeMap<String, Value>>,
    /// session_id -> (question_id -> QuestionRequest)
    pub questions: BTreeMap<String, BTreeMap<String, Value>>,
    pub vcs_branch: Option<String>,
}

impl ShadowStore {
    pub fn new(directory: impl Into<String>) -> Self {
        Self {
            directory: directory.into(),
            ..Default::default()
        }
    }

    pub fn load_snapshot(&mut self, snapshot: &TapeSnapshot, target_directory: Option<&str>) {
        if let Some(dir) = target_directory {
            self.directory = dir.to_string();
        }
        for proj in &snapshot.projects {
            let mut p = proj.clone();
            if let Some(dir) = target_directory {
                if let Some(obj) = p.as_object_mut() {
                    if obj.contains_key("worktree") {
                        obj.insert("worktree".into(), Value::String(dir.to_string()));
                    }
                }
            }
            if let Some(id) = p.get("id").and_then(Value::as_str) {
                self.projects.insert(id.to_string(), p);
            }
        }
        for sess in &snapshot.sessions {
            let mut s = sess.clone();
            if let Some(dir) = target_directory {
                if let Some(obj) = s.as_object_mut() {
                    obj.insert("directory".into(), Value::String(dir.to_string()));
                }
            }
            if let Some(id) = s.get("id").and_then(Value::as_str) {
                self.sessions.insert(id.to_string(), s);
            }
        }
        for (sid, msgs) in &snapshot.messages {
            let entry = self.messages.entry(sid.clone()).or_default();
            for m in msgs {
                if let Some(id) = m.get("id").and_then(Value::as_str) {
                    entry.insert(id.to_string(), m.clone());
                }
            }
        }
        for (mid, parts) in &snapshot.parts {
            let entry = self.parts.entry(mid.clone()).or_default();
            for p in parts {
                if let Some(id) = p.get("id").and_then(Value::as_str) {
                    entry.insert(id.to_string(), p.clone());
                }
            }
        }
        for (sid, st) in &snapshot.statuses {
            self.statuses.insert(sid.clone(), st.clone());
        }
        for (sid, td) in &snapshot.todos {
            self.todos.insert(sid.clone(), td.clone());
        }
        for (sid, df) in &snapshot.diffs {
            self.diffs.insert(sid.clone(), df.clone());
        }
    }

    pub fn apply_event(&mut self, payload: &Value) {
        let Some(event_type) = payload.get("type").and_then(Value::as_str) else {
            return;
        };
        let props = payload.get("properties").unwrap_or(&Value::Null);

        match event_type {
            "project.updated" => {
                if let Some(id) = props.get("id").and_then(Value::as_str) {
                    self.projects.insert(id.to_string(), props.clone());
                }
            }
            "session.created" | "session.updated" => {
                if let Some(info) = props.get("info") {
                    if let Some(id) = info.get("id").and_then(Value::as_str) {
                        let archived = info
                            .get("time")
                            .and_then(|t| t.get("archived"))
                            .is_some_and(|v| !v.is_null());
                        if archived {
                            self.drop_session(id);
                        } else {
                            self.sessions.insert(id.to_string(), info.clone());
                        }
                    }
                }
            }
            "session.deleted" => {
                if let Some(id) = props
                    .get("info")
                    .and_then(|i| i.get("id"))
                    .and_then(Value::as_str)
                {
                    self.drop_session(id);
                }
            }
            "session.status" => {
                if let (Some(sid), Some(status)) = (
                    props.get("sessionID").and_then(Value::as_str),
                    props.get("status"),
                ) {
                    self.statuses.insert(sid.to_string(), status.clone());
                }
            }
            "session.idle" => {
                if let Some(sid) = props.get("sessionID").and_then(Value::as_str) {
                    self.statuses
                        .insert(sid.to_string(), json!({ "type": "idle" }));
                }
            }
            "session.diff" => {
                if let (Some(sid), Some(diff)) = (
                    props.get("sessionID").and_then(Value::as_str),
                    props.get("diff").and_then(Value::as_array),
                ) {
                    self.diffs.insert(sid.to_string(), diff.clone());
                }
            }
            "todo.updated" => {
                if let (Some(sid), Some(todos)) = (
                    props.get("sessionID").and_then(Value::as_str),
                    props.get("todos").and_then(Value::as_array),
                ) {
                    self.todos.insert(sid.to_string(), todos.clone());
                }
            }
            "message.updated" => {
                if let Some(info) = props.get("info") {
                    if let (Some(sid), Some(mid)) = (
                        info.get("sessionID").and_then(Value::as_str),
                        info.get("id").and_then(Value::as_str),
                    ) {
                        self.messages
                            .entry(sid.to_string())
                            .or_default()
                            .insert(mid.to_string(), info.clone());
                    }
                }
            }
            "message.removed" => {
                if let (Some(sid), Some(mid)) = (
                    props.get("sessionID").and_then(Value::as_str),
                    props.get("messageID").and_then(Value::as_str),
                ) {
                    if let Some(msgs) = self.messages.get_mut(sid) {
                        msgs.remove(mid);
                    }
                    self.parts.remove(mid);
                }
            }
            "message.part.updated" => {
                if let Some(part) = props.get("part") {
                    if let (Some(mid), Some(pid)) = (
                        part.get("messageID").and_then(Value::as_str),
                        part.get("id").and_then(Value::as_str),
                    ) {
                        self.parts
                            .entry(mid.to_string())
                            .or_default()
                            .insert(pid.to_string(), part.clone());
                    }
                }
            }
            "message.part.removed" => {
                if let (Some(mid), Some(pid)) = (
                    props.get("messageID").and_then(Value::as_str),
                    props.get("partID").and_then(Value::as_str),
                ) {
                    if let Some(parts) = self.parts.get_mut(mid) {
                        parts.remove(pid);
                    }
                }
            }
            "message.part.delta" => {
                if let (Some(mid), Some(pid), Some(field), Some(delta)) = (
                    props.get("messageID").and_then(Value::as_str),
                    props.get("partID").and_then(Value::as_str),
                    props.get("field").and_then(Value::as_str),
                    props.get("delta").and_then(Value::as_str),
                ) {
                    if let Some(part) = self
                        .parts
                        .get_mut(mid)
                        .and_then(|map| map.get_mut(pid))
                        .and_then(Value::as_object_mut)
                    {
                        let existing = part
                            .get(field)
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        part.insert(field.to_string(), Value::String(format!("{existing}{delta}")));
                    }
                }
            }
            "permission.asked" => {
                if let (Some(sid), Some(id)) = (
                    props.get("sessionID").and_then(Value::as_str),
                    props.get("id").and_then(Value::as_str),
                ) {
                    self.permissions
                        .entry(sid.to_string())
                        .or_default()
                        .insert(id.to_string(), props.clone());
                }
            }
            "permission.replied" => {
                if let (Some(sid), Some(req_id)) = (
                    props.get("sessionID").and_then(Value::as_str),
                    props.get("requestID").and_then(Value::as_str),
                ) {
                    if let Some(map) = self.permissions.get_mut(sid) {
                        map.remove(req_id);
                    }
                }
            }
            "question.asked" => {
                if let (Some(sid), Some(id)) = (
                    props.get("sessionID").and_then(Value::as_str),
                    props.get("id").and_then(Value::as_str),
                ) {
                    self.questions
                        .entry(sid.to_string())
                        .or_default()
                        .insert(id.to_string(), props.clone());
                }
            }
            "question.replied" | "question.rejected" => {
                if let (Some(sid), Some(req_id)) = (
                    props.get("sessionID").and_then(Value::as_str),
                    props.get("requestID").and_then(Value::as_str),
                ) {
                    if let Some(map) = self.questions.get_mut(sid) {
                        map.remove(req_id);
                    }
                }
            }
            "vcs.branch.updated" => {
                if let Some(branch) = props.get("branch").and_then(Value::as_str) {
                    self.vcs_branch = Some(branch.to_string());
                }
            }
            _ => {}
        }
    }

    fn drop_session(&mut self, session_id: &str) {
        self.sessions.remove(session_id);
        if let Some(msgs) = self.messages.remove(session_id) {
            for mid in msgs.keys() {
                self.parts.remove(mid);
            }
        }
        self.statuses.remove(session_id);
        self.todos.remove(session_id);
        self.diffs.remove(session_id);
        self.permissions.remove(session_id);
        self.questions.remove(session_id);
    }

    pub fn sessions_list(&self) -> Vec<Value> {
        self.sessions.values().cloned().collect()
    }

    pub fn session_by_id(&self, id: &str) -> Option<Value> {
        self.sessions.get(id).cloned()
    }

    /// Returns `Array<{ info: Message, parts: Part[] }>` matching OpenCode `GET /session/:id/message`.
    pub fn messages_with_parts(&self, session_id: &str) -> Vec<Value> {
        let Some(msgs) = self.messages.get(session_id) else {
            return Vec::new();
        };
        msgs.iter()
            .map(|(mid, info)| {
                let parts: Vec<Value> = self
                    .parts
                    .get(mid)
                    .map(|p| p.values().cloned().collect())
                    .unwrap_or_default();
                json!({
                    "info": info,
                    "parts": parts,
                })
            })
            .collect()
    }

    pub fn status_map(&self) -> Value {
        let obj: serde_json::Map<String, Value> = self
            .statuses
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        Value::Object(obj)
    }

    pub fn todos_for(&self, session_id: &str) -> Vec<Value> {
        self.todos.get(session_id).cloned().unwrap_or_default()
    }

    pub fn diffs_for(&self, session_id: &str) -> Vec<Value> {
        self.diffs.get(session_id).cloned().unwrap_or_default()
    }

    pub fn all_permissions(&self) -> Vec<Value> {
        self.permissions
            .values()
            .flat_map(|m| m.values().cloned())
            .collect()
    }

    pub fn all_questions(&self) -> Vec<Value> {
        self.questions
            .values()
            .flat_map(|m| m.values().cloned())
            .collect()
    }

    pub fn projects_list(&self) -> Vec<Value> {
        if !self.projects.is_empty() {
            return self.projects.values().cloned().collect();
        }
        let dir = if self.directory.is_empty() {
            "/workspace/demo"
        } else {
            &self.directory
        };
        vec![json!({
            "id": "proj_replay_shadow",
            "worktree": dir,
            "vcs": "git",
            "time": { "created": now_ms(), "updated": now_ms() }
        })]
    }

    pub fn snapshot_json(&self, session_id: Option<&str>) -> Value {
        if let Some(sid) = session_id {
            json!({
                "directory": self.directory,
                "session": self.session_by_id(sid),
                "status": self.statuses.get(sid).cloned().unwrap_or_else(|| json!({ "type": "idle" })),
                "messages": self.messages_with_parts(sid),
                "todos": self.todos_for(sid),
                "diffs": self.diffs_for(sid),
                "permissions": self.permissions.get(sid).map(|m| m.values().cloned().collect::<Vec<_>>()).unwrap_or_default(),
                "questions": self.questions.get(sid).map(|m| m.values().cloned().collect::<Vec<_>>()).unwrap_or_default(),
            })
        } else {
            json!({
                "directory": self.directory,
                "projects": self.projects_list(),
                "sessions": self.sessions_list(),
                "statuses": self.status_map(),
                "permissions": self.all_permissions(),
                "questions": self.all_questions(),
            })
        }
    }

    /// Reconstruct client state from recorded SSE events up to `up_to_seq` (inclusive).
    pub fn reconstruct_from_records(
        events: &[SseRecord],
        up_to_seq: Option<i64>,
    ) -> Self {
        let dir = events
            .iter()
            .find(|e| !e.directory.is_empty() && e.directory != "global")
            .map(|e| e.directory.clone())
            .unwrap_or_default();
        let mut store = Self::new(dir);
        for ev in events {
            if let Some(limit) = up_to_seq {
                if ev.seq > limit {
                    break;
                }
            }
            store.apply_event(&ev.payload);
        }
        store
    }
}

/// Remap directory fields inside an OpenCode SSE event payload (e.g., `session.created/updated`
/// `properties.info.directory` and `project.updated` `properties.worktree`) so the connected
/// OpenCode client accepts the replayed event under its active workspace directory.
pub fn remap_event_directory(payload: &Value, target_dir: &str) -> Value {
    let mut out = payload.clone();
    let Some(event_type) = out.get("type").and_then(Value::as_str).map(ToOwned::to_owned) else {
        return out;
    };
    if let Some(props) = out.get_mut("properties").and_then(Value::as_object_mut) {
        match event_type.as_str() {
            "session.created" | "session.updated" | "session.deleted" => {
                if let Some(info) = props.get_mut("info").and_then(Value::as_object_mut) {
                    info.insert("directory".into(), Value::String(target_dir.to_string()));
                }
            }
            "project.updated" => {
                props.insert("worktree".into(), Value::String(target_dir.to_string()));
            }
            _ => {}
        }
    }
    out
}
