use crate::model::*;
use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct Storage {
    conn: Arc<Mutex<Connection>>,
}

impl Storage {
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        if let Some(parent) = path.as_ref().parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let conn = Connection::open(path)?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        Self::init(conn)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "busy_timeout", 5000)?;

        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS http_traces (
                id TEXT PRIMARY KEY,
                started_at INTEGER NOT NULL,
                completed_at INTEGER,
                duration_ms INTEGER,
                method TEXT NOT NULL,
                path TEXT NOT NULL,
                query TEXT,
                directory TEXT,
                session_id TEXT,
                status INTEGER,
                req_headers TEXT NOT NULL,
                req_body TEXT,
                req_size INTEGER NOT NULL DEFAULT 0,
                res_headers TEXT NOT NULL DEFAULT '{}',
                res_body TEXT,
                res_size INTEGER NOT NULL DEFAULT 0,
                mocked INTEGER NOT NULL DEFAULT 0,
                mock_rule_id TEXT,
                upstream_error TEXT,
                pending INTEGER NOT NULL DEFAULT 1
            );
            CREATE INDEX IF NOT EXISTS idx_http_started ON http_traces(started_at DESC);
            CREATE INDEX IF NOT EXISTS idx_http_session ON http_traces(session_id, started_at);
            CREATE INDEX IF NOT EXISTS idx_http_status ON http_traces(status);

            CREATE TABLE IF NOT EXISTS sse_events (
                seq INTEGER PRIMARY KEY AUTOINCREMENT,
                ts INTEGER NOT NULL,
                source TEXT NOT NULL,
                directory TEXT NOT NULL,
                event_type TEXT NOT NULL,
                session_id TEXT,
                message_id TEXT,
                part_id TEXT,
                payload_json TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_sse_ts ON sse_events(ts);
            CREATE INDEX IF NOT EXISTS idx_sse_session ON sse_events(session_id, seq);
            CREATE INDEX IF NOT EXISTS idx_sse_type ON sse_events(event_type);

            CREATE VIRTUAL TABLE IF NOT EXISTS traffic_fts USING fts5(
                kind UNINDEXED,
                ref_id UNINDEXED,
                ts UNINDEXED,
                session_id UNINDEXED,
                meta,
                content
            );

            CREATE TABLE IF NOT EXISTS mock_rules (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                enabled INTEGER NOT NULL DEFAULT 1,
                priority INTEGER NOT NULL DEFAULT 100,
                method TEXT NOT NULL,
                path_pattern TEXT NOT NULL,
                directory_filter TEXT,
                action_type TEXT NOT NULL,
                status INTEGER NOT NULL DEFAULT 200,
                delay_ms INTEGER NOT NULL DEFAULT 0,
                headers_json TEXT NOT NULL DEFAULT '{}',
                body_json TEXT,
                patch_json TEXT,
                created_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS tapes (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                description TEXT,
                directory TEXT NOT NULL,
                session_id TEXT,
                snapshot_json TEXT NOT NULL,
                events_json TEXT NOT NULL,
                created_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS ignored_sessions (
                session_id TEXT PRIMARY KEY,
                created_at INTEGER NOT NULL
            );
            "#,
        )?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub fn insert_http_started(&self, trace: &HttpTrace) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let req_headers = serde_json::to_string(&trace.req_headers)?;
        let res_headers = serde_json::to_string(&trace.res_headers)?;
        conn.execute(
            r#"INSERT INTO http_traces (
                id, started_at, completed_at, duration_ms, method, path, query, directory,
                session_id, status, req_headers, req_body, req_size, res_headers, res_body,
                res_size, mocked, mock_rule_id, upstream_error, pending
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)"#,
            params![
                trace.id,
                trace.started_at,
                trace.completed_at,
                trace.duration_ms,
                trace.method,
                trace.path,
                trace.query,
                trace.directory,
                trace.session_id,
                trace.status.map(i64::from),
                req_headers,
                trace.req_body,
                trace.req_size as i64,
                res_headers,
                trace.res_body,
                trace.res_size as i64,
                trace.mocked as i64,
                trace.mock_rule_id,
                trace.upstream_error,
                trace.pending as i64,
            ],
        )?;
        Ok(())
    }

    pub fn complete_http_trace(&self, trace: &HttpTrace) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let req_headers = serde_json::to_string(&trace.req_headers)?;
        let res_headers = serde_json::to_string(&trace.res_headers)?;
        conn.execute(
            r#"UPDATE http_traces SET
                completed_at = ?2,
                duration_ms = ?3,
                session_id = COALESCE(?4, session_id),
                status = ?5,
                req_headers = ?6,
                req_body = ?7,
                req_size = ?8,
                res_headers = ?9,
                res_body = ?10,
                res_size = ?11,
                mocked = ?12,
                mock_rule_id = ?13,
                upstream_error = ?14,
                pending = 0
            WHERE id = ?1"#,
            params![
                trace.id,
                trace.completed_at,
                trace.duration_ms,
                trace.session_id,
                trace.status.map(i64::from),
                req_headers,
                trace.req_body,
                trace.req_size as i64,
                res_headers,
                trace.res_body,
                trace.res_size as i64,
                trace.mocked as i64,
                trace.mock_rule_id,
                trace.upstream_error,
            ],
        )?;

        let meta = format!(
            "{} {} {} {}",
            trace.method,
            trace.path,
            trace.status.map(|s| s.to_string()).unwrap_or_default(),
            trace.directory.as_deref().unwrap_or("")
        );
        let mut content = String::new();
        if let Some(q) = &trace.query {
            content.push_str(q);
            content.push('\n');
        }
        if let Some(rb) = &trace.req_body {
            content.push_str(rb);
            content.push('\n');
        }
        if let Some(sb) = &trace.res_body {
            content.push_str(sb);
        }
        if let Some(err) = &trace.upstream_error {
            content.push('\n');
            content.push_str(err);
        }

        let _ = conn.execute(
            "INSERT INTO traffic_fts (kind, ref_id, ts, session_id, meta, content) VALUES ('http', ?1, ?2, ?3, ?4, ?5)",
            params![
                trace.id,
                trace.started_at,
                trace.session_id.as_deref().unwrap_or(""),
                meta,
                content,
            ],
        );
        Ok(())
    }

    pub fn insert_sse_event(
        &self,
        ts: i64,
        source: SseSource,
        directory: &str,
        payload: &Value,
    ) -> Result<SseRecord> {
        let event_type = payload
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        let (session_id, message_id, part_id) = extract_event_ids(payload);
        let payload_json = serde_json::to_string(payload)?;

        let conn = self.conn.lock().unwrap();
        conn.execute(
            r#"INSERT INTO sse_events (
                ts, source, directory, event_type, session_id, message_id, part_id, payload_json
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)"#,
            params![
                ts,
                source.as_str(),
                directory,
                event_type,
                session_id,
                message_id,
                part_id,
                payload_json,
            ],
        )?;
        let seq = conn.last_insert_rowid();

        // Skip indexing heartbeats in FTS to keep index lean
        if event_type != "server.heartbeat" {
            let meta = format!("{} {}", event_type, directory);
            let _ = conn.execute(
                "INSERT INTO traffic_fts (kind, ref_id, ts, session_id, meta, content) VALUES ('sse', ?1, ?2, ?3, ?4, ?5)",
                params![
                    seq.to_string(),
                    ts,
                    session_id.as_deref().unwrap_or(""),
                    meta,
                    payload_json,
                ],
            );
        }

        Ok(SseRecord {
            seq,
            ts,
            source,
            directory: directory.to_string(),
            event_type,
            session_id,
            message_id,
            part_id,
            payload: payload.clone(),
        })
    }

    pub fn list_http_traces(
        &self,
        session_id: Option<&str>,
        method: Option<&str>,
        status_min: Option<u16>,
        exclude_session_id: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<HttpTraceSummary>> {
        self.query_http_traces(
            session_id,
            method,
            status_min,
            exclude_session_id,
            Some(limit.max(1)),
            offset,
        )
    }

    pub fn list_all_http_traces(
        &self,
        session_id: Option<&str>,
        method: Option<&str>,
        status_min: Option<u16>,
        exclude_session_id: Option<&str>,
    ) -> Result<Vec<HttpTraceSummary>> {
        self.query_http_traces(session_id, method, status_min, exclude_session_id, None, 0)
    }

    fn query_http_traces(
        &self,
        session_id: Option<&str>,
        method: Option<&str>,
        status_min: Option<u16>,
        exclude_session_id: Option<&str>,
        limit: Option<usize>,
        offset: usize,
    ) -> Result<Vec<HttpTraceSummary>> {
        let conn = self.conn.lock().unwrap();
        let mut sql = String::from(
            r#"SELECT id, started_at, duration_ms, method, path, query, directory, session_id,
                      status, req_size, res_size, mocked, mock_rule_id, upstream_error, pending
               FROM http_traces WHERE 1=1"#,
        );
        let mut dyn_params: Vec<rusqlite::types::Value> = Vec::new();
        if let Some(sid) = session_id {
            sql.push_str(" AND session_id = ?");
            dyn_params.push(sid.to_string().into());
        }
        if let Some(ex) = exclude_session_id {
            sql.push_str(" AND (session_id IS NULL OR session_id != ?)");
            dyn_params.push(ex.to_string().into());
        }
        if let Some(m) = method {
            sql.push_str(" AND UPPER(method) = UPPER(?)");
            dyn_params.push(m.to_string().into());
        }
        if let Some(smin) = status_min {
            sql.push_str(" AND status >= ?");
            dyn_params.push((smin as i64).into());
        }
        sql.push_str(" ORDER BY started_at DESC");
        if let Some(lim) = limit {
            sql.push_str(" LIMIT ? OFFSET ?");
            dyn_params.push((lim as i64).into());
            dyn_params.push((offset as i64).into());
        }

        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(dyn_params), |row| {
            Ok(HttpTraceSummary {
                id: row.get(0)?,
                started_at: row.get(1)?,
                duration_ms: row.get(2)?,
                method: row.get(3)?,
                path: row.get(4)?,
                query: row.get(5)?,
                directory: row.get(6)?,
                session_id: row.get(7)?,
                status: row.get::<_, Option<i64>>(8)?.map(|v| v as u16),
                req_size: row.get::<_, i64>(9)? as usize,
                res_size: row.get::<_, i64>(10)? as usize,
                mocked: row.get::<_, i64>(11)? != 0,
                mock_rule_id: row.get(12)?,
                upstream_error: row.get(13)?,
                pending: row.get::<_, i64>(14)? != 0,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn get_http_trace(&self, id: &str) -> Result<Option<HttpTrace>> {
        let conn = self.conn.lock().unwrap();
        let row = conn
            .query_row(
                r#"SELECT id, started_at, completed_at, duration_ms, method, path, query, directory,
                          session_id, status, req_headers, req_body, req_size, res_headers, res_body,
                          res_size, mocked, mock_rule_id, upstream_error, pending
                   FROM http_traces WHERE id = ?1"#,
                params![id],
                |row| {
                    let req_h: String = row.get(10)?;
                    let res_h: String = row.get(13)?;
                    Ok(HttpTrace {
                        id: row.get(0)?,
                        started_at: row.get(1)?,
                        completed_at: row.get(2)?,
                        duration_ms: row.get(3)?,
                        method: row.get(4)?,
                        path: row.get(5)?,
                        query: row.get(6)?,
                        directory: row.get(7)?,
                        session_id: row.get(8)?,
                        status: row.get::<_, Option<i64>>(9)?.map(|v| v as u16),
                        req_headers: serde_json::from_str(&req_h).unwrap_or_default(),
                        req_body: row.get(11)?,
                        req_size: row.get::<_, i64>(12)? as usize,
                        res_headers: serde_json::from_str(&res_h).unwrap_or_default(),
                        res_body: row.get(14)?,
                        res_size: row.get::<_, i64>(15)? as usize,
                        mocked: row.get::<_, i64>(16)? != 0,
                        mock_rule_id: row.get(17)?,
                        upstream_error: row.get(18)?,
                        pending: row.get::<_, i64>(19)? != 0,
                    })
                },
            )
            .optional()?;
        Ok(row)
    }

    pub fn list_sse_events(
        &self,
        session_id: Option<&str>,
        event_type: Option<&str>,
        directory: Option<&str>,
        from_seq: Option<i64>,
        to_seq: Option<i64>,
        exclude_heartbeat: bool,
        exclude_session_id: Option<&str>,
        limit: usize,
        descending: bool,
    ) -> Result<Vec<SseRecord>> {
        self.query_sse_events(
            session_id,
            event_type,
            directory,
            from_seq,
            to_seq,
            exclude_heartbeat,
            exclude_session_id,
            Some(limit.max(1)),
            descending,
        )
    }

    /// Fetch all matching SSE events in ascending `seq` order without the single-page query cap.
    pub fn list_all_sse_events(
        &self,
        session_id: Option<&str>,
        event_type: Option<&str>,
        directory: Option<&str>,
        from_seq: Option<i64>,
        to_seq: Option<i64>,
        exclude_heartbeat: bool,
        exclude_session_id: Option<&str>,
    ) -> Result<Vec<SseRecord>> {
        self.query_sse_events(
            session_id,
            event_type,
            directory,
            from_seq,
            to_seq,
            exclude_heartbeat,
            exclude_session_id,
            None,
            false,
        )
    }

    fn query_sse_events(
        &self,
        session_id: Option<&str>,
        event_type: Option<&str>,
        directory: Option<&str>,
        from_seq: Option<i64>,
        to_seq: Option<i64>,
        exclude_heartbeat: bool,
        exclude_session_id: Option<&str>,
        limit: Option<usize>,
        descending: bool,
    ) -> Result<Vec<SseRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut sql = String::from(
            r#"SELECT seq, ts, source, directory, event_type, session_id, message_id, part_id, payload_json
               FROM sse_events WHERE 1=1"#,
        );
        let mut dyn_params: Vec<rusqlite::types::Value> = Vec::new();
        if let Some(sid) = session_id {
            sql.push_str(" AND session_id = ?");
            dyn_params.push(sid.to_string().into());
        }
        if let Some(ex) = exclude_session_id {
            sql.push_str(" AND (session_id IS NULL OR session_id != ?)");
            dyn_params.push(ex.to_string().into());
        }
        if let Some(et) = event_type {
            sql.push_str(" AND event_type = ?");
            dyn_params.push(et.to_string().into());
        }
        if let Some(dir) = directory {
            sql.push_str(" AND directory = ?");
            dyn_params.push(dir.to_string().into());
        }
        if let Some(fs) = from_seq {
            sql.push_str(" AND seq >= ?");
            dyn_params.push(fs.into());
        }
        if let Some(ts) = to_seq {
            sql.push_str(" AND seq <= ?");
            dyn_params.push(ts.into());
        }
        if exclude_heartbeat {
            sql.push_str(" AND event_type != 'server.heartbeat'");
        }
        if descending {
            sql.push_str(" ORDER BY seq DESC");
        } else {
            sql.push_str(" ORDER BY seq ASC");
        }
        if let Some(lim) = limit {
            sql.push_str(" LIMIT ?");
            dyn_params.push((lim as i64).into());
        }

        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(dyn_params), |row| {
            let src: String = row.get(2)?;
            let raw: String = row.get(8)?;
            Ok(SseRecord {
                seq: row.get(0)?,
                ts: row.get(1)?,
                source: SseSource::parse(&src),
                directory: row.get(3)?,
                event_type: row.get(4)?,
                session_id: row.get(5)?,
                message_id: row.get(6)?,
                part_id: row.get(7)?,
                payload: serde_json::from_str(&raw).unwrap_or(Value::Null),
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    /// Reconstruct client ShadowStore state for `session_id` up to `at_seq` (inclusive) by
    /// streaming all matching SSE events in sequence order without any page-size truncation.
    pub fn reconstruct_session_state(
        &self,
        session_id: &str,
        at_seq: Option<i64>,
    ) -> Result<crate::shadow::ShadowStore> {
        let conn = self.conn.lock().unwrap();
        let mut sql = String::from(
            r#"SELECT directory, payload_json
               FROM sse_events
               WHERE session_id = ? AND event_type != 'server.heartbeat'"#,
        );
        let mut dyn_params: Vec<rusqlite::types::Value> = vec![session_id.to_string().into()];
        if let Some(ts) = at_seq {
            sql.push_str(" AND seq <= ?");
            dyn_params.push(ts.into());
        }
        sql.push_str(" ORDER BY seq ASC");

        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(dyn_params), |row| {
            let dir: String = row.get(0)?;
            let raw: String = row.get(1)?;
            Ok((dir, raw))
        })?;

        let mut store = crate::shadow::ShadowStore::new(String::new());
        for row in rows {
            let (dir, raw) = row?;
            if store.directory.is_empty() && !dir.is_empty() && dir != "global" {
                store.directory = dir;
            }
            if let Ok(payload) = serde_json::from_str::<Value>(&raw) {
                store.apply_event(&payload);
            }
        }
        Ok(store)
    }

    pub fn list_sessions(
        &self,
        exclude_session_id: Option<&str>,
        include_ignored: bool,
    ) -> Result<Vec<SessionOverview>> {
        let conn = self.conn.lock().unwrap();
        let ignored: HashSet<String> = {
            let mut stmt = conn.prepare("SELECT session_id FROM ignored_sessions")?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            rows.filter_map(Result::ok).collect()
        };

        let mut map: HashMap<String, SessionOverview> = HashMap::new();

        {
            let mut stmt = conn.prepare(
                r#"SELECT session_id, directory, MIN(started_at), MAX(started_at),
                          COUNT(*),
                          SUM(CASE WHEN status >= 400 OR upstream_error IS NOT NULL THEN 1 ELSE 0 END)
                   FROM http_traces
                   WHERE session_id IS NOT NULL AND session_id != ''
                   GROUP BY session_id"#,
            )?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, i64>(4)? as usize,
                    r.get::<_, i64>(5)? as usize,
                ))
            })?;
            for row in rows {
                let (sid, dir, first_ts, last_ts, count, err_count) = row?;
                map.insert(
                    sid.clone(),
                    SessionOverview {
                        session_id: sid.clone(),
                        directory: dir,
                        title: None,
                        first_ts,
                        last_ts,
                        http_count: count,
                        http_error_count: err_count,
                        sse_count: 0,
                        ignored: ignored.contains(&sid),
                    },
                );
            }
        }

        {
            let mut stmt = conn.prepare(
                r#"SELECT session_id, directory, MIN(ts), MAX(ts), COUNT(*)
                   FROM sse_events
                   WHERE session_id IS NOT NULL AND session_id != ''
                   GROUP BY session_id"#,
            )?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, i64>(4)? as usize,
                ))
            })?;
            for row in rows {
                let (sid, dir, first_ts, last_ts, count) = row?;
                let entry = map.entry(sid.clone()).or_insert_with(|| SessionOverview {
                    session_id: sid.clone(),
                    directory: if dir.is_empty() || dir == "global" {
                        None
                    } else {
                        Some(dir.clone())
                    },
                    title: None,
                    first_ts,
                    last_ts,
                    http_count: 0,
                    http_error_count: 0,
                    sse_count: 0,
                    ignored: ignored.contains(&sid),
                });
                entry.sse_count = count;
                entry.first_ts = entry.first_ts.min(first_ts);
                entry.last_ts = entry.last_ts.max(last_ts);
                if entry.directory.is_none() && !dir.is_empty() && dir != "global" {
                    entry.directory = Some(dir);
                }
            }
        }

        // Fill latest session title from session.created / session.updated events
        {
            let mut stmt = conn.prepare(
                r#"SELECT session_id, payload_json FROM sse_events
                   WHERE event_type IN ('session.created', 'session.updated')
                     AND session_id IS NOT NULL
                   ORDER BY seq ASC"#,
            )?;
            let rows = stmt.query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (sid, raw) = row?;
                if let Some(entry) = map.get_mut(&sid) {
                    if let Ok(val) = serde_json::from_str::<Value>(&raw) {
                        if let Some(title) = val
                            .get("properties")
                            .and_then(|p| p.get("info"))
                            .and_then(|i| i.get("title"))
                            .and_then(Value::as_str)
                        {
                            entry.title = Some(title.to_string());
                        }
                    }
                }
            }
        }

        let mut out: Vec<SessionOverview> = map
            .into_values()
            .filter(|s| {
                if let Some(ex) = exclude_session_id {
                    if s.session_id == ex {
                        return false;
                    }
                }
                if !include_ignored && s.ignored {
                    return false;
                }
                true
            })
            .collect();
        out.sort_by(|a, b| b.last_ts.cmp(&a.last_ts));
        Ok(out)
    }

    pub fn set_session_ignored(&self, session_id: &str, ignored: bool) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        if ignored {
            conn.execute(
                "INSERT OR REPLACE INTO ignored_sessions (session_id, created_at) VALUES (?1, ?2)",
                params![session_id, now_ms()],
            )?;
        } else {
            conn.execute(
                "DELETE FROM ignored_sessions WHERE session_id = ?1",
                params![session_id],
            )?;
        }
        Ok(())
    }

    /// Build a unified HTTP + SSE timeline for a session, with optional server-side delta folding.
    pub fn get_session_timeline(
        &self,
        session_id: &str,
        fold_deltas: bool,
        include_heartbeat: bool,
        limit: usize,
    ) -> Result<Vec<TimelineItem>> {
        let mut traces =
            self.list_all_http_traces(Some(session_id), None, None, None)?;
        traces.reverse();
        let events = self.list_all_sse_events(
            Some(session_id),
            None,
            None,
            None,
            None,
            !include_heartbeat,
            None,
        )?;

        let mut items: Vec<TimelineItem> = Vec::new();
        for t in traces {
            items.push(TimelineItem::Http {
                ts: t.started_at,
                trace: t,
            });
        }

        if fold_deltas {
            items.extend(fold_sse_events(events));
        } else {
            for e in events {
                items.push(TimelineItem::Sse { ts: e.ts, event: e });
            }
        }

        items.sort_by_key(|item| match item {
            TimelineItem::Http { ts, .. } => *ts,
            TimelineItem::Sse { ts, .. } => *ts,
            TimelineItem::FoldedDelta { ts, .. } => *ts,
        });
        if items.len() > limit {
            items.truncate(limit);
        }
        Ok(items)
    }

    /// Full-text search across HTTP traces and SSE payloads via SQLite FTS5.
    pub fn search_traffic(
        &self,
        query: &str,
        session_id: Option<&str>,
        kind: Option<&str>,
        exclude_session_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<SearchHit>> {
        let conn = self.conn.lock().unwrap();
        let sanitized = sanitize_fts_query(query);
        if sanitized.is_empty() {
            return Ok(Vec::new());
        }

        let mut sql = String::from(
            r#"SELECT kind, ref_id, ts, session_id,
                      snippet(traffic_fts, 5, '[', ']', '...', 24)
               FROM traffic_fts
               WHERE traffic_fts MATCH ?"#,
        );
        let mut dyn_params: Vec<rusqlite::types::Value> = vec![sanitized.into()];
        if let Some(sid) = session_id {
            sql.push_str(" AND session_id = ?");
            dyn_params.push(sid.to_string().into());
        }
        if let Some(ex) = exclude_session_id {
            sql.push_str(" AND session_id != ?");
            dyn_params.push(ex.to_string().into());
        }
        if let Some(k) = kind {
            sql.push_str(" AND kind = ?");
            dyn_params.push(k.to_string().into());
        }
        sql.push_str(" ORDER BY ts DESC LIMIT ?");
        dyn_params.push((limit.max(1) as i64).into());

        let mut stmt = conn.prepare(&sql)?;
        let raw_hits: Vec<(String, String, i64, String, String)> = stmt
            .query_map(rusqlite::params_from_iter(dyn_params), |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        let mut results = Vec::with_capacity(raw_hits.len());
        for (hit_kind, ref_id, ts, sid, snippet) in raw_hits {
            let sid_opt = if sid.is_empty() { None } else { Some(sid) };
            if hit_kind == "http" {
                let row: Option<(String, String, Option<i64>)> = conn
                    .query_row(
                        "SELECT method, path, status FROM http_traces WHERE id = ?1",
                        params![ref_id],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                    )
                    .optional()?;
                if let Some((method, path, status)) = row {
                    results.push(SearchHit::Http {
                        id: ref_id,
                        ts,
                        method,
                        path,
                        status: status.map(|v| v as u16),
                        session_id: sid_opt,
                        snippet,
                    });
                }
            } else if let Ok(seq) = ref_id.parse::<i64>() {
                let row: Option<(String, String)> = conn
                    .query_row(
                        "SELECT event_type, directory FROM sse_events WHERE seq = ?1",
                        params![seq],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()?;
                if let Some((event_type, directory)) = row {
                    results.push(SearchHit::Sse {
                        seq,
                        ts,
                        event_type,
                        session_id: sid_opt,
                        directory,
                        snippet,
                    });
                }
            }
        }
        Ok(results)
    }

    pub fn clear_traffic(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(
            r#"
            DELETE FROM http_traces;
            DELETE FROM sse_events;
            DELETE FROM traffic_fts;
            "#,
        )?;
        Ok(())
    }

    // ---- Mock Rules ----

    pub fn list_mock_rules(&self) -> Result<Vec<MockRule>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"SELECT id, name, enabled, priority, method, path_pattern, directory_filter,
                      action_type, status, delay_ms, headers_json, body_json, patch_json, created_at
               FROM mock_rules ORDER BY priority ASC, created_at DESC"#,
        )?;
        let rows = stmt.query_map([], |r| {
            let action_str: String = r.get(7)?;
            let headers_str: String = r.get(10)?;
            let body_str: Option<String> = r.get(11)?;
            let patch_str: Option<String> = r.get(12)?;
            Ok(MockRule {
                id: r.get(0)?,
                name: r.get(1)?,
                enabled: r.get::<_, i64>(2)? != 0,
                priority: r.get(3)?,
                method: r.get(4)?,
                path_pattern: r.get(5)?,
                directory_filter: r.get(6)?,
                action_type: MockActionType::parse(&action_str),
                status: r.get::<_, i64>(8)? as u16,
                delay_ms: r.get::<_, i64>(9)? as u64,
                headers: serde_json::from_str(&headers_str).unwrap_or_default(),
                body: body_str.and_then(|s| serde_json::from_str(&s).ok()),
                patch: patch_str.and_then(|s| serde_json::from_str(&s).ok()),
                created_at: r.get(13)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn upsert_mock_rule(&self, rule: &MockRule) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let headers_json = serde_json::to_string(&rule.headers)?;
        let body_json = rule
            .body
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        let patch_json = rule
            .patch
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        conn.execute(
            r#"INSERT OR REPLACE INTO mock_rules (
                id, name, enabled, priority, method, path_pattern, directory_filter,
                action_type, status, delay_ms, headers_json, body_json, patch_json, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)"#,
            params![
                rule.id,
                rule.name,
                rule.enabled as i64,
                rule.priority,
                rule.method,
                rule.path_pattern,
                rule.directory_filter,
                rule.action_type.as_str(),
                rule.status as i64,
                rule.delay_ms as i64,
                headers_json,
                body_json,
                patch_json,
                rule.created_at,
            ],
        )?;
        Ok(())
    }

    pub fn delete_mock_rule(&self, id: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let affected = conn.execute("DELETE FROM mock_rules WHERE id = ?1", params![id])?;
        Ok(affected > 0)
    }

    pub fn find_matching_rule(
        &self,
        method: &str,
        path: &str,
        directory: Option<&str>,
    ) -> Result<Option<MockRule>> {
        let rules = self.list_mock_rules()?;
        for rule in rules {
            if !rule.enabled {
                continue;
            }
            if rule.method != "ANY" && !rule.method.eq_ignore_ascii_case(method) {
                continue;
            }
            if !path_matches(&rule.path_pattern, path) {
                continue;
            }
            if let Some(dir_filter) = &rule.directory_filter {
                if !dir_filter.is_empty() && directory != Some(dir_filter.as_str()) {
                    continue;
                }
            }
            return Ok(Some(rule));
        }
        Ok(None)
    }

    // ---- Tapes ----

    pub fn list_tapes(&self) -> Result<Vec<TapeSummary>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, description, directory, session_id, events_json, created_at FROM tapes ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            let events_str: String = r.get(5)?;
            let events: Vec<TapeEvent> = serde_json::from_str(&events_str).unwrap_or_default();
            let duration_ms = events.last().map(|e| e.offset_ms).unwrap_or(0);
            Ok(TapeSummary {
                id: r.get(0)?,
                name: r.get(1)?,
                description: r.get(2)?,
                directory: r.get(3)?,
                session_id: r.get(4)?,
                event_count: events.len(),
                duration_ms,
                created_at: r.get(6)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn get_tape(&self, id: &str) -> Result<Option<Tape>> {
        let conn = self.conn.lock().unwrap();
        let row = conn
            .query_row(
                "SELECT id, name, description, directory, session_id, snapshot_json, events_json, created_at FROM tapes WHERE id = ?1",
                params![id],
                |r| {
                    let snap_str: String = r.get(5)?;
                    let ev_str: String = r.get(6)?;
                    Ok(Tape {
                        id: r.get(0)?,
                        name: r.get(1)?,
                        description: r.get(2)?,
                        directory: r.get(3)?,
                        session_id: r.get(4)?,
                        snapshot: serde_json::from_str(&snap_str).unwrap_or_default(),
                        events: serde_json::from_str(&ev_str).unwrap_or_default(),
                        created_at: r.get(7)?,
                    })
                },
            )
            .optional()?;
        Ok(row)
    }

    pub fn save_tape(&self, tape: &Tape) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let snap_json = serde_json::to_string(&tape.snapshot)?;
        let events_json = serde_json::to_string(&tape.events)?;
        conn.execute(
            r#"INSERT OR REPLACE INTO tapes (
                id, name, description, directory, session_id, snapshot_json, events_json, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)"#,
            params![
                tape.id,
                tape.name,
                tape.description,
                tape.directory,
                tape.session_id,
                snap_json,
                events_json,
                tape.created_at,
            ],
        )?;
        Ok(())
    }

    pub fn delete_tape(&self, id: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let affected = conn.execute("DELETE FROM tapes WHERE id = ?1", params![id])?;
        Ok(affected > 0)
    }

    /// Extract a replayable Tape from recorded traffic for a given session ID.
    pub fn extract_tape_from_session(
        &self,
        session_id: &str,
        name: Option<String>,
    ) -> Result<Tape> {
        let events = self.list_all_sse_events(
            Some(session_id),
            None,
            None,
            None,
            None,
            true,
            None,
        )?;
        let first_ts = events.first().map(|e| e.ts).unwrap_or_else(now_ms);
        let directory = events
            .iter()
            .find(|e| !e.directory.is_empty() && e.directory != "global")
            .map(|e| e.directory.clone())
            .unwrap_or_else(|| "/workspace/project".to_string());

        let mut snapshot = TapeSnapshot::default();
        // Pull initial session info from earliest GET /session/:id trace if available
        let target_path = format!("/session/{session_id}");
        let earliest_body: Option<String> = {
            let conn = self.conn.lock().unwrap();
            conn.query_row(
                r#"SELECT res_body FROM http_traces
                   WHERE session_id = ?1 AND UPPER(method) = 'GET' AND path = ?2 AND res_body IS NOT NULL
                   ORDER BY started_at ASC LIMIT 1"#,
                params![session_id, target_path],
                |row| row.get(0),
            )
            .optional()?
        };
        if let Some(body) = earliest_body {
            if let Ok(val) = serde_json::from_str::<Value>(&body) {
                if val.get("id").is_some() {
                    snapshot.sessions = vec![val];
                }
            }
        }

        let tape_events: Vec<TapeEvent> = events
            .into_iter()
            .filter(|e| e.event_type != "server.connected" && e.event_type != "server.heartbeat")
            .map(|e| TapeEvent {
                offset_ms: (e.ts - first_ts).max(0) as u64,
                directory: e.directory,
                payload: e.payload,
            })
            .collect();

        let tape = Tape {
            id: format!("tape_{}", &uuid::Uuid::new_v4().simple().to_string()[..10]),
            name: name.unwrap_or_else(|| format!("Recorded {session_id}")),
            description: Some(format!(
                "Extracted from recorded traffic for session {session_id} ({} events)",
                tape_events.len()
            )),
            directory,
            session_id: Some(session_id.to_string()),
            snapshot,
            events: tape_events,
            created_at: now_ms(),
        };
        self.save_tape(&tape)
            .context("failed to persist extracted tape")?;
        Ok(tape)
    }
}

/// Fold consecutive `message.part.delta` SSE events targeting the same `(messageID, partID, field)`
/// into compact summary items to save >95% context tokens for DevTools and MCP agents.
pub fn fold_sse_events(events: Vec<SseRecord>) -> Vec<TimelineItem> {
    let mut out = Vec::new();

    struct PendingFold {
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
        accumulated: String,
    }

    let mut current: Option<PendingFold> = None;

    let flush = |cur: &mut Option<PendingFold>, target: &mut Vec<TimelineItem>| {
        if let Some(f) = cur.take() {
            let preview = truncate_str(&f.accumulated, 160);
            target.push(TimelineItem::FoldedDelta {
                ts: f.ts,
                end_ts: f.end_ts,
                from_seq: f.from_seq,
                to_seq: f.to_seq,
                count: f.count,
                directory: f.directory,
                session_id: f.session_id,
                message_id: f.message_id,
                part_id: f.part_id,
                field: f.field,
                total_chars: f.accumulated.chars().count(),
                duration_ms: (f.end_ts - f.ts).max(0),
                preview,
            });
        }
    };

    for ev in events {
        if ev.event_type == "message.part.delta" {
            let props = ev.payload.get("properties");
            let mid = props
                .and_then(|p| p.get("messageID"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let pid = props
                .and_then(|p| p.get("partID"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let field = props
                .and_then(|p| p.get("field"))
                .and_then(Value::as_str)
                .unwrap_or("text")
                .to_string();
            let delta = props
                .and_then(|p| p.get("delta"))
                .and_then(Value::as_str)
                .unwrap_or("");

            let same_group = current.as_ref().is_some_and(|f| {
                f.message_id == mid && f.part_id == pid && f.field == field
            });

            if same_group {
                if let Some(f) = current.as_mut() {
                    f.end_ts = ev.ts;
                    f.to_seq = ev.seq;
                    f.count += 1;
                    f.accumulated.push_str(delta);
                }
            } else {
                flush(&mut current, &mut out);
                current = Some(PendingFold {
                    ts: ev.ts,
                    end_ts: ev.ts,
                    from_seq: ev.seq,
                    to_seq: ev.seq,
                    count: 1,
                    directory: ev.directory,
                    session_id: ev.session_id,
                    message_id: mid,
                    part_id: pid,
                    field,
                    accumulated: delta.to_string(),
                });
            }
        } else {
            flush(&mut current, &mut out);
            out.push(TimelineItem::Sse {
                ts: ev.ts,
                event: ev,
            });
        }
    }
    flush(&mut current, &mut out);
    out
}

fn sanitize_fts_query(raw: &str) -> String {
    let tokens: Vec<String> = raw
        .split_whitespace()
        .map(|tok| {
            let clean: String = tok
                .chars()
                .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-' || *c == '.' || *c == '/')
                .collect();
            clean
        })
        .filter(|s| !s.is_empty())
        .map(|s| format!("\"{}\"", s.replace('"', "")))
        .collect();
    tokens.join(" ")
}

pub fn truncate_str(s: &str, max_chars: usize) -> String {
    let mut chars = s.chars();
    let collected: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{collected}...")
    } else {
        collected
    }
}
