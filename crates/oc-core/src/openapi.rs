use crate::model::*;
use anyhow::{anyhow, Result};
use serde_json::{json, Map, Value};
use std::collections::HashSet;

#[derive(Debug, Clone)]
pub struct OpenApiRegistry {
    pub source: String,
    pub doc: Value,
}

impl Default for OpenApiRegistry {
    fn default() -> Self {
        Self {
            source: "builtin".to_string(),
            doc: builtin_opencode_openapi(),
        }
    }
}

impl OpenApiRegistry {
    pub fn from_doc(source: impl Into<String>, doc: Value) -> Self {
        Self {
            source: source.into(),
            doc,
        }
    }

    pub async fn fetch_from_upstream(upstream_url: &str) -> Result<Self> {
        let url = format!("{}/doc", upstream_url.trim_end_matches('/'));
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(8))
            .build()?;
        let res = client.get(&url).send().await?;
        if !res.status().is_success() {
            return Err(anyhow!("upstream /doc returned HTTP {}", res.status()));
        }
        let doc: Value = res.json().await?;
        Ok(Self::from_doc(format!("upstream:{url}"), doc))
    }

    pub fn summary(&self) -> OpenApiSummary {
        let title = self
            .doc
            .pointer("/info/title")
            .and_then(Value::as_str)
            .unwrap_or("opencode")
            .to_string();
        let version = self
            .doc
            .pointer("/info/version")
            .and_then(Value::as_str)
            .unwrap_or("0.0.0")
            .to_string();

        OpenApiSummary {
            source: self.source.clone(),
            title,
            version,
            operations: self.operations(),
            events: self.event_schemas(),
        }
    }

    pub fn operations(&self) -> Vec<OpenApiOperation> {
        let mut ops = Vec::new();
        let Some(paths) = self.doc.get("paths").and_then(Value::as_object) else {
            return ops;
        };
        for (path, item) in paths {
            let Some(methods) = item.as_object() else {
                continue;
            };
            for (method, spec) in methods {
                let m = method.to_uppercase();
                if !matches!(m.as_str(), "GET" | "POST" | "PUT" | "PATCH" | "DELETE") {
                    continue;
                }
                let operation_id = spec
                    .get("operationId")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned);
                let summary = spec
                    .get("summary")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned);
                let description = spec
                    .get("description")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned);
                let response_schema = spec
                    .pointer("/responses/200/content/application~1json/schema")
                    .or_else(|| spec.pointer("/responses/200/content/text~1event-stream/schema"))
                    .cloned();
                let request_schema = spec
                    .pointer("/requestBody/content/application~1json/schema")
                    .cloned();
                ops.push(OpenApiOperation {
                    method: m,
                    path: normalize_openapi_path(path),
                    operation_id,
                    summary,
                    description,
                    response_schema,
                    request_schema,
                });
            }
        }
        ops.sort_by(|a, b| a.path.cmp(&b.path).then_with(|| a.method.cmp(&b.method)));
        ops
    }

    pub fn find_operation(&self, method: &str, path: &str) -> Option<OpenApiOperation> {
        let norm = normalize_openapi_path(path);
        self.operations().into_iter().find(|op| {
            op.method.eq_ignore_ascii_case(method)
                && (op.path == norm || path_matches(&op.path, path))
        })
    }

    pub fn event_schemas(&self) -> Vec<OpenApiEventSchema> {
        let mut out = Vec::new();
        let event_root = self
            .doc
            .pointer("/components/schemas/Event")
            .or_else(|| self.doc.pointer("/paths/~1event/get/responses/200/content/text~1event-stream/schema"));

        let Some(root) = event_root else {
            return out;
        };
        let resolved = self.resolve_ref(root).unwrap_or(root);
        let variants = resolved
            .get("oneOf")
            .or_else(|| resolved.get("anyOf"))
            .and_then(Value::as_array);

        if let Some(list) = variants {
            for item in list {
                let target = self.resolve_ref(item).unwrap_or(item);
                let event_type = target
                    .pointer("/properties/type/const")
                    .or_else(|| target.pointer("/properties/type/enum/0"))
                    .and_then(Value::as_str);
                if let Some(et) = event_type {
                    out.push(OpenApiEventSchema {
                        event_type: et.to_string(),
                        schema: target.clone(),
                    });
                }
            }
        }
        out.sort_by(|a, b| a.event_type.cmp(&b.event_type));
        out
    }

    pub fn find_event_schema(&self, event_type: &str) -> Option<Value> {
        self.event_schemas()
            .into_iter()
            .find(|e| e.event_type == event_type)
            .map(|e| e.schema)
    }

    pub fn fake_http_response(&self, method: &str, path: &str, directory: &str) -> Result<Value> {
        let op = self
            .find_operation(method, path)
            .ok_or_else(|| anyhow!("no OpenAPI operation found for {} {}", method, path))?;
        let Some(schema) = op.response_schema else {
            return Ok(json!({ "ok": true }));
        };
        Ok(self.fake_value(&schema, directory, None, 0))
    }

    pub fn fake_sse_event(&self, event_type: &str, directory: &str) -> Result<Value> {
        let schema = self
            .find_event_schema(event_type)
            .ok_or_else(|| anyhow!("no OpenAPI event schema found for '{}'", event_type))?;
        Ok(self.fake_value(&schema, directory, None, 0))
    }

    pub fn resolve_ref<'a>(&'a self, schema: &'a Value) -> Option<&'a Value> {
        let r = schema.get("$ref")?.as_str()?;
        let ptr = r.strip_prefix('#')?;
        self.doc.pointer(ptr)
    }

    pub fn bundle_schema(&self, schema: &Value) -> Value {
        let mut bundled = schema.clone();
        if let (Some(obj), Some(components)) = (bundled.as_object_mut(), self.doc.get("components"))
        {
            obj.insert("components".to_string(), components.clone());
        }
        bundled
    }

    pub fn validate(&self, schema: &Value, instance: &Value) -> Result<(), Vec<String>> {
        let bundled = self.bundle_schema(schema);
        let validator = match jsonschema::validator_for(&bundled) {
            Ok(v) => v,
            Err(err) => return Err(vec![format!("invalid JSON schema: {err}")]),
        };
        let errors: Vec<String> = validator
            .iter_errors(instance)
            .take(12)
            .map(|e| format!("{} (at {})", e, e.instance_path))
            .collect();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    pub fn fake_value(
        &self,
        schema: &Value,
        directory: &str,
        field_name: Option<&str>,
        depth: usize,
    ) -> Value {
        if depth > 8 {
            return Value::Null;
        }
        if let Some(target) = self.resolve_ref(schema) {
            return self.fake_value(target, directory, field_name, depth + 1);
        }
        if let Some(c) = schema.get("const") {
            return c.clone();
        }
        if let Some(e) = schema.get("enum").and_then(Value::as_array) {
            if let Some(first) = e.first() {
                return first.clone();
            }
        }
        if let Some(default_val) = schema.get("default") {
            return default_val.clone();
        }
        if let Some(one_of) = schema
            .get("oneOf")
            .or_else(|| schema.get("anyOf"))
            .and_then(Value::as_array)
        {
            if let Some(first) = one_of.first() {
                return self.fake_value(first, directory, field_name, depth + 1);
            }
        }
        if let Some(all_of) = schema.get("allOf").and_then(Value::as_array) {
            let mut merged = Map::new();
            for sub in all_of {
                if let Value::Object(map) = self.fake_value(sub, directory, field_name, depth + 1) {
                    for (k, v) in map {
                        merged.insert(k, v);
                    }
                }
            }
            return Value::Object(merged);
        }

        let ty = schema.get("type").and_then(Value::as_str).unwrap_or_else(|| {
            if schema.get("properties").is_some() {
                "object"
            } else if schema.get("items").is_some() {
                "array"
            } else {
                "string"
            }
        });

        match ty {
            "object" => {
                let mut map = Map::new();
                if let Some(props) = schema.get("properties").and_then(Value::as_object) {
                    let required: HashSet<&str> = schema
                        .get("required")
                        .and_then(Value::as_array)
                        .map(|arr| arr.iter().filter_map(Value::as_str).collect())
                        .unwrap_or_default();
                    for (k, v) in props {
                        if required.is_empty() || required.contains(k.as_str()) || depth <= 2 {
                            map.insert(k.clone(), self.fake_value(v, directory, Some(k), depth + 1));
                        }
                    }
                } else if let Some(add_props) = schema.get("additionalProperties") {
                    if add_props.is_object() {
                        map.insert(
                            "ses_mock_001".to_string(),
                            self.fake_value(add_props, directory, Some("item"), depth + 1),
                        );
                    }
                }
                Value::Object(map)
            }
            "array" => {
                if let Some(items) = schema.get("items") {
                    Value::Array(vec![self.fake_value(items, directory, field_name, depth + 1)])
                } else {
                    Value::Array(Vec::new())
                }
            }
            "integer" => {
                let name = field_name.unwrap_or("");
                if matches!(name, "created" | "updated" | "start" | "end" | "next" | "ts") {
                    json!(now_ms())
                } else if name == "port" {
                    json!(4096)
                } else {
                    json!(1)
                }
            }
            "number" => json!(0.01),
            "boolean" => json!(true),
            "null" => Value::Null,
            _ => Value::String(fake_string(field_name.unwrap_or("value"), directory)),
        }
    }
}

fn fake_string(field: &str, directory: &str) -> String {
    let f = field.to_lowercase();
    let dir = if directory.is_empty() {
        "/workspace/project"
    } else {
        directory
    };
    if f == "directory" || f == "worktree" || f == "cwd" || f == "root" {
        return dir.to_string();
    }
    if f == "sessionid" {
        return "ses_mock_001".to_string();
    }
    if f == "messageid" || f == "parentid" {
        return "msg_mock_001".to_string();
    }
    if f == "partid" {
        return "prt_mock_001".to_string();
    }
    if f == "projectid" {
        return "proj_mock_001".to_string();
    }
    if f == "requestid" {
        return "req_mock_001".to_string();
    }
    if f == "callid" {
        return "call_mock_001".to_string();
    }
    if f == "id" {
        return format!("id_mock_{}", &uuid::Uuid::new_v4().simple().to_string()[..6]);
    }
    if f == "providerid" {
        return "anthropic".to_string();
    }
    if f == "modelid" {
        return "claude-sonnet-4".to_string();
    }
    if f == "agent" || f == "mode" {
        return "build".to_string();
    }
    if f == "role" {
        return "assistant".to_string();
    }
    if f == "branch" {
        return "main".to_string();
    }
    if f == "version" {
        return "1.0.0".to_string();
    }
    if f == "slug" {
        return "mock-session".to_string();
    }
    if f == "title" {
        return "Mock OpenCode Session".to_string();
    }
    if f == "field" {
        return "text".to_string();
    }
    if f == "delta" || f == "text" || f == "content" || f == "message" {
        return "Mock generated OpenCode content".to_string();
    }
    if f == "file" || f == "path" {
        return "src/main.rs".to_string();
    }
    format!("mock_{field}")
}

/// Convert OpenAPI `{sessionID}` params to `:sessionID` style matching our rule matcher.
pub fn normalize_openapi_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    let mut chars = path.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '{' {
            out.push(':');
            for inner in chars.by_ref() {
                if inner == '}' {
                    break;
                }
                out.push(inner);
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Validate semantic invariants of a generated or imported Session Tape:
/// - non-empty events, monotonic `offset_ms`
/// - every event has a string `type`
/// - every `message.part.delta` has a preceding `message.part.updated` (or snapshot part)
pub fn validate_tape_semantics(tape: &Tape) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();
    if tape.events.is_empty() {
        errors.push("tape contains zero events".to_string());
    }

    let mut seen_parts: HashSet<(String, String)> = HashSet::new();
    for (mid, parts) in &tape.snapshot.parts {
        for p in parts {
            if let Some(pid) = p.get("id").and_then(Value::as_str) {
                seen_parts.insert((mid.clone(), pid.to_string()));
            }
        }
    }

    let mut prev_offset = 0u64;
    for (idx, ev) in tape.events.iter().enumerate() {
        if idx > 0 && ev.offset_ms < prev_offset {
            errors.push(format!(
                "event #{idx}: offset_ms ({}) decreased below previous ({})",
                ev.offset_ms, prev_offset
            ));
        }
        prev_offset = ev.offset_ms;

        let Some(et) = ev.payload.get("type").and_then(Value::as_str) else {
            errors.push(format!("event #{idx}: missing string payload.type"));
            continue;
        };

        let (_, mid, pid) = extract_event_ids(&ev.payload);
        if et == "message.part.updated" {
            if let (Some(m), Some(p)) = (mid, pid) {
                seen_parts.insert((m, p));
            } else {
                errors.push(format!(
                    "event #{idx}: message.part.updated missing properties.part.messageID or id"
                ));
            }
        } else if et == "message.part.delta" {
            match (mid, pid) {
                (Some(m), Some(p)) => {
                    if !seen_parts.contains(&(m.clone(), p.clone())) {
                        errors.push(format!(
                            "event #{idx}: message.part.delta references ({m}, {p}) before any message.part.updated"
                        ));
                    }
                }
                _ => {
                    errors.push(format!(
                        "event #{idx}: message.part.delta missing messageID or partID"
                    ));
                }
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Built-in core OpenCode OpenAPI 3.1.1 spec so Faker, Schema Explorer, and LLM generator
/// work seamlessly even without a live upstream OpenCode server.
pub fn builtin_opencode_openapi() -> Value {
    json!({
        "openapi": "3.1.1",
        "info": {
            "title": "opencode (built-in core spec)",
            "version": "1.0.0",
            "description": "Built-in OpenCode REST + BusEvent OpenAPI specification"
        },
        "paths": {
            "/global/health": {
                "get": {
                    "operationId": "global.health",
                    "summary": "Get health",
                    "responses": {
                        "200": {
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "object",
                                        "required": ["healthy", "version"],
                                        "properties": {
                                            "healthy": { "const": true },
                                            "version": { "type": "string" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            },
            "/path": {
                "get": {
                    "operationId": "path.get",
                    "summary": "Get instance paths",
                    "responses": {
                        "200": {
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/Path" }
                                }
                            }
                        }
                    }
                }
            },
            "/project": {
                "get": {
                    "operationId": "project.list",
                    "summary": "List projects",
                    "responses": {
                        "200": {
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "array",
                                        "items": { "$ref": "#/components/schemas/Project" }
                                    }
                                }
                            }
                        }
                    }
                }
            },
            "/project/current": {
                "get": {
                    "operationId": "project.current",
                    "summary": "Get current project",
                    "responses": {
                        "200": {
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/Project" }
                                }
                            }
                        }
                    }
                }
            },
            "/session": {
                "get": {
                    "operationId": "session.list",
                    "summary": "List sessions",
                    "responses": {
                        "200": {
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "array",
                                        "items": { "$ref": "#/components/schemas/Session" }
                                    }
                                }
                            }
                        }
                    }
                },
                "post": {
                    "operationId": "session.create",
                    "summary": "Create session",
                    "responses": {
                        "200": {
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/Session" }
                                }
                            }
                        }
                    }
                }
            },
            "/session/status": {
                "get": {
                    "operationId": "session.status",
                    "summary": "Get session status map",
                    "responses": {
                        "200": {
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "object",
                                        "additionalProperties": { "$ref": "#/components/schemas/SessionStatus" }
                                    }
                                }
                            }
                        }
                    }
                }
            },
            "/session/{sessionID}": {
                "get": {
                    "operationId": "session.get",
                    "summary": "Get session details",
                    "responses": {
                        "200": {
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/Session" }
                                }
                            }
                        }
                    }
                }
            },
            "/session/{sessionID}/message": {
                "get": {
                    "operationId": "session.messages",
                    "summary": "List session messages with parts",
                    "responses": {
                        "200": {
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "array",
                                        "items": { "$ref": "#/components/schemas/MessageWithParts" }
                                    }
                                }
                            }
                        }
                    }
                },
                "post": {
                    "operationId": "session.prompt",
                    "summary": "Send prompt message (streaming JSON)",
                    "responses": {
                        "200": {
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/MessageWithParts" }
                                }
                            }
                        }
                    }
                }
            },
            "/session/{sessionID}/todo": {
                "get": {
                    "operationId": "session.todo",
                    "summary": "Get session todos",
                    "responses": {
                        "200": {
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "array",
                                        "items": { "$ref": "#/components/schemas/Todo" }
                                    }
                                }
                            }
                        }
                    }
                }
            },
            "/session/{sessionID}/diff": {
                "get": {
                    "operationId": "session.diff",
                    "summary": "Get session file diffs",
                    "responses": {
                        "200": {
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "array",
                                        "items": { "$ref": "#/components/schemas/FileDiff" }
                                    }
                                }
                            }
                        }
                    }
                }
            },
            "/permission": {
                "get": {
                    "operationId": "permission.list",
                    "summary": "List pending permissions",
                    "responses": {
                        "200": {
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "array",
                                        "items": { "$ref": "#/components/schemas/PermissionRequest" }
                                    }
                                }
                            }
                        }
                    }
                }
            },
            "/question": {
                "get": {
                    "operationId": "question.list",
                    "summary": "List pending questions",
                    "responses": {
                        "200": {
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "array",
                                        "items": { "$ref": "#/components/schemas/QuestionRequest" }
                                    }
                                }
                            }
                        }
                    }
                }
            },
            "/vcs": {
                "get": {
                    "operationId": "vcs.get",
                    "summary": "Get VCS branch info",
                    "responses": {
                        "200": {
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "object",
                                        "required": ["branch"],
                                        "properties": { "branch": { "type": "string" } }
                                    }
                                }
                            }
                        }
                    }
                }
            },
            "/event": {
                "get": {
                    "operationId": "event.subscribe",
                    "summary": "Subscribe to directory SSE events",
                    "responses": {
                        "200": {
                            "content": {
                                "text/event-stream": {
                                    "schema": { "$ref": "#/components/schemas/Event" }
                                }
                            }
                        }
                    }
                }
            },
            "/global/event": {
                "get": {
                    "operationId": "global.event",
                    "summary": "Subscribe to global SSE events",
                    "responses": {
                        "200": {
                            "content": {
                                "text/event-stream": {
                                    "schema": {
                                        "type": "object",
                                        "required": ["directory", "payload"],
                                        "properties": {
                                            "directory": { "type": "string" },
                                            "payload": { "$ref": "#/components/schemas/Event" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        },
        "components": {
            "schemas": {
                "Path": {
                    "type": "object",
                    "required": ["home", "state", "config", "worktree", "directory"],
                    "properties": {
                        "home": { "type": "string" },
                        "state": { "type": "string" },
                        "config": { "type": "string" },
                        "worktree": { "type": "string" },
                        "directory": { "type": "string" }
                    }
                },
                "Project": {
                    "type": "object",
                    "required": ["id", "worktree", "time"],
                    "properties": {
                        "id": { "type": "string" },
                        "worktree": { "type": "string" },
                        "vcs": { "type": "string", "enum": ["git"] },
                        "time": {
                            "type": "object",
                            "required": ["created", "updated"],
                            "properties": {
                                "created": { "type": "integer" },
                                "updated": { "type": "integer" }
                            }
                        }
                    }
                },
                "Session": {
                    "type": "object",
                    "required": ["id", "slug", "projectID", "directory", "title", "version", "time"],
                    "properties": {
                        "id": { "type": "string" },
                        "slug": { "type": "string" },
                        "projectID": { "type": "string" },
                        "directory": { "type": "string" },
                        "title": { "type": "string" },
                        "version": { "type": "string" },
                        "time": {
                            "type": "object",
                            "required": ["created", "updated"],
                            "properties": {
                                "created": { "type": "integer" },
                                "updated": { "type": "integer" }
                            }
                        }
                    }
                },
                "SessionStatus": {
                    "oneOf": [
                        { "type": "object", "required": ["type"], "properties": { "type": { "const": "idle" } } },
                        { "type": "object", "required": ["type"], "properties": { "type": { "const": "busy" } } },
                        {
                            "type": "object",
                            "required": ["type", "attempt", "message", "next"],
                            "properties": {
                                "type": { "const": "retry" },
                                "attempt": { "type": "integer" },
                                "message": { "type": "string" },
                                "next": { "type": "integer" }
                            }
                        }
                    ]
                },
                "Message": {
                    "type": "object",
                    "required": ["id", "sessionID", "role", "time"],
                    "properties": {
                        "id": { "type": "string" },
                        "sessionID": { "type": "string" },
                        "role": { "type": "string", "enum": ["assistant", "user"] },
                        "agent": { "type": "string" },
                        "providerID": { "type": "string" },
                        "modelID": { "type": "string" },
                        "time": {
                            "type": "object",
                            "required": ["created"],
                            "properties": { "created": { "type": "integer" } }
                        }
                    }
                },
                "Part": {
                    "type": "object",
                    "required": ["id", "sessionID", "messageID", "type"],
                    "properties": {
                        "id": { "type": "string" },
                        "sessionID": { "type": "string" },
                        "messageID": { "type": "string" },
                        "type": { "type": "string", "enum": ["text", "reasoning", "tool"] },
                        "text": { "type": "string" }
                    }
                },
                "MessageWithParts": {
                    "type": "object",
                    "required": ["info", "parts"],
                    "properties": {
                        "info": { "$ref": "#/components/schemas/Message" },
                        "parts": { "type": "array", "items": { "$ref": "#/components/schemas/Part" } }
                    }
                },
                "Todo": {
                    "type": "object",
                    "required": ["id", "content", "status", "priority"],
                    "properties": {
                        "id": { "type": "string" },
                        "content": { "type": "string" },
                        "status": { "type": "string", "enum": ["pending", "in_progress", "completed", "cancelled"] },
                        "priority": { "type": "string", "enum": ["high", "medium", "low"] }
                    }
                },
                "FileDiff": {
                    "type": "object",
                    "required": ["file", "before", "after", "additions", "deletions"],
                    "properties": {
                        "file": { "type": "string" },
                        "before": { "type": "string" },
                        "after": { "type": "string" },
                        "additions": { "type": "integer" },
                        "deletions": { "type": "integer" }
                    }
                },
                "PermissionRequest": {
                    "type": "object",
                    "required": ["id", "sessionID", "permission", "patterns", "metadata", "always"],
                    "properties": {
                        "id": { "type": "string" },
                        "sessionID": { "type": "string" },
                        "permission": { "type": "string", "default": "bash" },
                        "patterns": { "type": "array", "items": { "type": "string" } },
                        "metadata": { "type": "object" },
                        "always": { "type": "array", "items": { "type": "string" } }
                    }
                },
                "QuestionRequest": {
                    "type": "object",
                    "required": ["id", "sessionID", "questions"],
                    "properties": {
                        "id": { "type": "string" },
                        "sessionID": { "type": "string" },
                        "questions": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "required": ["question", "header", "options"],
                                "properties": {
                                    "question": { "type": "string" },
                                    "header": { "type": "string" },
                                    "options": {
                                        "type": "array",
                                        "items": {
                                            "type": "object",
                                            "required": ["label", "description"],
                                            "properties": {
                                                "label": { "type": "string" },
                                                "description": { "type": "string" }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                },
                "Event": {
                    "oneOf": [
                        {
                            "type": "object",
                            "required": ["type", "properties"],
                            "properties": {
                                "type": { "const": "session.created" },
                                "properties": {
                                    "type": "object",
                                    "required": ["info"],
                                    "properties": { "info": { "$ref": "#/components/schemas/Session" } }
                                }
                            }
                        },
                        {
                            "type": "object",
                            "required": ["type", "properties"],
                            "properties": {
                                "type": { "const": "session.updated" },
                                "properties": {
                                    "type": "object",
                                    "required": ["info"],
                                    "properties": { "info": { "$ref": "#/components/schemas/Session" } }
                                }
                            }
                        },
                        {
                            "type": "object",
                            "required": ["type", "properties"],
                            "properties": {
                                "type": { "const": "session.status" },
                                "properties": {
                                    "type": "object",
                                    "required": ["sessionID", "status"],
                                    "properties": {
                                        "sessionID": { "type": "string" },
                                        "status": { "$ref": "#/components/schemas/SessionStatus" }
                                    }
                                }
                            }
                        },
                        {
                            "type": "object",
                            "required": ["type", "properties"],
                            "properties": {
                                "type": { "const": "session.idle" },
                                "properties": {
                                    "type": "object",
                                    "required": ["sessionID"],
                                    "properties": { "sessionID": { "type": "string" } }
                                }
                            }
                        },
                        {
                            "type": "object",
                            "required": ["type", "properties"],
                            "properties": {
                                "type": { "const": "session.diff" },
                                "properties": {
                                    "type": "object",
                                    "required": ["sessionID", "diff"],
                                    "properties": {
                                        "sessionID": { "type": "string" },
                                        "diff": { "type": "array", "items": { "$ref": "#/components/schemas/FileDiff" } }
                                    }
                                }
                            }
                        },
                        {
                            "type": "object",
                            "required": ["type", "properties"],
                            "properties": {
                                "type": { "const": "session.error" },
                                "properties": {
                                    "type": "object",
                                    "required": ["sessionID", "error"],
                                    "properties": {
                                        "sessionID": { "type": "string" },
                                        "error": {
                                            "type": "object",
                                            "required": ["name", "data"],
                                            "properties": {
                                                "name": { "type": "string", "default": "UnknownError" },
                                                "data": { "type": "object", "properties": { "message": { "type": "string" } } }
                                            }
                                        }
                                    }
                                }
                            }
                        },
                        {
                            "type": "object",
                            "required": ["type", "properties"],
                            "properties": {
                                "type": { "const": "message.updated" },
                                "properties": {
                                    "type": "object",
                                    "required": ["info"],
                                    "properties": { "info": { "$ref": "#/components/schemas/Message" } }
                                }
                            }
                        },
                        {
                            "type": "object",
                            "required": ["type", "properties"],
                            "properties": {
                                "type": { "const": "message.part.updated" },
                                "properties": {
                                    "type": "object",
                                    "required": ["part"],
                                    "properties": { "part": { "$ref": "#/components/schemas/Part" } }
                                }
                            }
                        },
                        {
                            "type": "object",
                            "required": ["type", "properties"],
                            "properties": {
                                "type": { "const": "message.part.delta" },
                                "properties": {
                                    "type": "object",
                                    "required": ["sessionID", "messageID", "partID", "field", "delta"],
                                    "properties": {
                                        "sessionID": { "type": "string" },
                                        "messageID": { "type": "string" },
                                        "partID": { "type": "string" },
                                        "field": { "type": "string" },
                                        "delta": { "type": "string" }
                                    }
                                }
                            }
                        },
                        {
                            "type": "object",
                            "required": ["type", "properties"],
                            "properties": {
                                "type": { "const": "todo.updated" },
                                "properties": {
                                    "type": "object",
                                    "required": ["sessionID", "todos"],
                                    "properties": {
                                        "sessionID": { "type": "string" },
                                        "todos": { "type": "array", "items": { "$ref": "#/components/schemas/Todo" } }
                                    }
                                }
                            }
                        },
                        {
                            "type": "object",
                            "required": ["type", "properties"],
                            "properties": {
                                "type": { "const": "permission.asked" },
                                "properties": { "$ref": "#/components/schemas/PermissionRequest" }
                            }
                        },
                        {
                            "type": "object",
                            "required": ["type", "properties"],
                            "properties": {
                                "type": { "const": "permission.replied" },
                                "properties": {
                                    "type": "object",
                                    "required": ["sessionID", "requestID", "reply"],
                                    "properties": {
                                        "sessionID": { "type": "string" },
                                        "requestID": { "type": "string" },
                                        "reply": { "type": "string", "enum": ["once", "always", "reject"] }
                                    }
                                }
                            }
                        },
                        {
                            "type": "object",
                            "required": ["type", "properties"],
                            "properties": {
                                "type": { "const": "question.asked" },
                                "properties": { "$ref": "#/components/schemas/QuestionRequest" }
                            }
                        },
                        {
                            "type": "object",
                            "required": ["type", "properties"],
                            "properties": {
                                "type": { "const": "tui.session.select" },
                                "properties": {
                                    "type": "object",
                                    "required": ["sessionID"],
                                    "properties": { "sessionID": { "type": "string" } }
                                }
                            }
                        },
                        {
                            "type": "object",
                            "required": ["type", "properties"],
                            "properties": {
                                "type": { "const": "tui.toast.show" },
                                "properties": {
                                    "type": "object",
                                    "required": ["message", "variant"],
                                    "properties": {
                                        "title": { "type": "string" },
                                        "message": { "type": "string" },
                                        "variant": { "type": "string", "enum": ["info", "success", "warning", "error"] }
                                    }
                                }
                            }
                        }
                    ]
                }
            }
        }
    })
}
