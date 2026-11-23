use dashmap::DashMap;
use oc_core::openapi::OpenApiRegistry;
use oc_core::tape::builtin_demo_tape;
use oc_core::*;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use tokio::sync::{broadcast, mpsc, oneshot};

#[derive(Debug, Clone)]
pub struct ProxyConfig {
    pub proxy_addr: String,
    pub control_addr: String,
    pub upstream_url: Option<String>,
    pub default_directory: String,
    pub web_app_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SseClientKind {
    Global,
    Directory(String),
}

#[derive(Debug, Clone)]
pub struct OutgoingSseFrame {
    pub data: String,
}

pub struct SseClientEntry {
    pub kind: SseClientKind,
    pub tx: mpsc::Sender<OutgoingSseFrame>,
}

pub struct PendingBreakpointEntry {
    pub info: PendingBreakpoint,
    pub tx: oneshot::Sender<BreakpointDecision>,
}

pub struct ReplayRuntime {
    pub tape: Option<Tape>,
    pub status: ReplayStatus,
    pub task: Option<tokio::task::AbortHandle>,
}

impl Default for ReplayRuntime {
    fn default() -> Self {
        Self {
            tape: None,
            status: ReplayStatus::default(),
            task: None,
        }
    }
}

pub struct AppState {
    pub config: ProxyConfig,
    pub storage: Storage,
    pub http_client: reqwest::Client,
    pub mode: RwLock<ProxyMode>,
    pub mute_upstream_sse: AtomicBool,
    pub recording: AtomicBool,
    pub active_directories: DashMap<String, i64>,
    pub sse_clients: DashMap<String, SseClientEntry>,
    pub breakpoints: DashMap<String, PendingBreakpointEntry>,
    pub openapi: RwLock<OpenApiRegistry>,
    pub shadow: RwLock<ShadowStore>,
    pub replay: Mutex<ReplayRuntime>,
    pub ws_tx: broadcast::Sender<ControlWsEvent>,
    pub llm_config: RwLock<Option<LlmConfig>>,
}

impl AppState {
    pub fn new(config: ProxyConfig, storage: Storage) -> Arc<Self> {
        // Seed built-in demo tape if no tapes exist yet
        if let Ok(tapes) = storage.list_tapes() {
            if tapes.is_empty() {
                let demo = builtin_demo_tape(&config.default_directory);
                let _ = storage.save_tape(&demo);
            }
        }

        let http_client = reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("failed to build reqwest client");

        let (ws_tx, _) = broadcast::channel(1024);

        let initial_llm = match (
            std::env::var("OC_LLM_API_BASE").ok(),
            std::env::var("OC_LLM_MODEL").ok(),
        ) {
            (Some(api_base), Some(model)) if !api_base.is_empty() && !model.is_empty() => {
                Some(LlmConfig {
                    api_base,
                    api_key: std::env::var("OC_LLM_API_KEY").unwrap_or_default(),
                    model,
                    temperature: 0.3,
                })
            }
            _ => None,
        };

        let default_dir = config.default_directory.clone();
        let state = Arc::new(Self {
            config,
            storage,
            http_client,
            mode: RwLock::new(ProxyMode::Passthrough),
            mute_upstream_sse: AtomicBool::new(false),
            recording: AtomicBool::new(true),
            active_directories: DashMap::new(),
            sse_clients: DashMap::new(),
            breakpoints: DashMap::new(),
            openapi: RwLock::new(OpenApiRegistry::default()),
            shadow: RwLock::new(ShadowStore::new(default_dir.clone())),
            replay: Mutex::new(ReplayRuntime::default()),
            ws_tx,
            llm_config: RwLock::new(initial_llm),
        });
        state.active_directories.insert(default_dir, now_ms());
        state
    }

    pub fn record_directory(&self, dir: &str) {
        if !dir.is_empty() && dir != "global" {
            self.active_directories.insert(dir.to_string(), now_ms());
        }
    }

    pub fn primary_active_directory(&self) -> String {
        self.active_directories
            .iter()
            .max_by_key(|e| *e.value())
            .map(|e| e.key().clone())
            .unwrap_or_else(|| self.config.default_directory.clone())
    }

    pub fn active_directory_list(&self) -> Vec<String> {
        let mut items: Vec<(String, i64)> = self
            .active_directories
            .iter()
            .map(|e| (e.key().clone(), *e.value()))
            .collect();
        items.sort_by(|a, b| b.1.cmp(&a.1));
        items.into_iter().map(|(k, _)| k).collect()
    }

    pub fn mode(&self) -> ProxyMode {
        *self.mode.read().unwrap()
    }

    pub fn set_mode(&self, next: ProxyMode) {
        *self.mode.write().unwrap() = next;
        self.broadcast_status();
    }

    pub fn is_muted(&self) -> bool {
        self.mute_upstream_sse.load(Ordering::Relaxed)
    }

    pub fn set_muted(&self, muted: bool) {
        self.mute_upstream_sse.store(muted, Ordering::Relaxed);
        self.broadcast_status();
    }

    pub fn is_recording(&self) -> bool {
        self.recording.load(Ordering::Relaxed)
    }

    pub fn set_recording(&self, rec: bool) {
        self.recording.store(rec, Ordering::Relaxed);
        self.broadcast_status();
    }

    pub fn runtime_status(&self) -> ProxyRuntimeStatus {
        let replay = self.replay.lock().unwrap().status.clone();
        let openapi_source = self.openapi.read().unwrap().source.clone();
        ProxyRuntimeStatus {
            proxy_addr: self.config.proxy_addr.clone(),
            control_addr: self.config.control_addr.clone(),
            upstream_url: self.config.upstream_url.clone(),
            mode: self.mode(),
            mute_upstream_sse: self.is_muted(),
            recording: self.is_recording(),
            active_directories: self.active_directory_list(),
            connected_sse_clients: self.sse_clients.len(),
            pending_breakpoints: self.breakpoints.len(),
            openapi_source,
            replay,
        }
    }

    pub fn broadcast(&self, event: ControlWsEvent) {
        let _ = self.ws_tx.send(event);
    }

    pub fn broadcast_status(&self) {
        self.broadcast(ControlWsEvent::Status {
            status: self.runtime_status(),
        });
    }

    /// Record (if enabled) and fan out an SSE event to all connected OpenCode clients
    /// (`/global/event` receives `{ directory, payload }`, `/event` receives `payload` filtered by directory).
    pub fn emit_sse(&self, source: SseSource, directory: &str, payload: Value) -> Option<SseRecord> {
        let event_type = payload
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("unknown");

        let recorded = if self.is_recording() || source != SseSource::Upstream {
            if event_type != "server.heartbeat" {
                match self
                    .storage
                    .insert_sse_event(now_ms(), source.clone(), directory, &payload)
                {
                    Ok(rec) => {
                        self.broadcast(ControlWsEvent::SseRecorded { event: rec.clone() });
                        Some(rec)
                    }
                    Err(err) => {
                        tracing::warn!("failed to record SSE event: {err}");
                        None
                    }
                }
            } else {
                None
            }
        } else {
            None
        };

        let global_data = serde_json::to_string(&json!({
            "directory": directory,
            "payload": payload,
        }))
        .unwrap_or_default();
        let direct_data = serde_json::to_string(&payload).unwrap_or_default();

        let mut dead_ids = Vec::new();
        for entry in self.sse_clients.iter() {
            let send_str = match &entry.value().kind {
                SseClientKind::Global => Some(&global_data),
                SseClientKind::Directory(client_dir) => {
                    if directory == "global"
                        || directory.is_empty()
                        || client_dir.is_empty()
                        || client_dir == directory
                    {
                        Some(&direct_data)
                    } else {
                        None
                    }
                }
            };
            if let Some(data) = send_str {
                if entry
                    .value()
                    .tx
                    .try_send(OutgoingSseFrame { data: data.clone() })
                    .is_err()
                {
                    if entry.value().tx.is_closed() {
                        dead_ids.push(entry.key().clone());
                    }
                }
            }
        }
        for id in dead_ids {
            self.sse_clients.remove(&id);
        }

        recorded
    }

    /// Push a raw control frame (such as `server.connected`) to all SSE clients without recording.
    pub fn broadcast_raw_sse(&self, directory: &str, payload: Value) {
        let global_data = serde_json::to_string(&json!({
            "directory": directory,
            "payload": payload,
        }))
        .unwrap_or_default();
        let direct_data = serde_json::to_string(&payload).unwrap_or_default();

        for entry in self.sse_clients.iter() {
            let data = match &entry.value().kind {
                SseClientKind::Global => global_data.clone(),
                SseClientKind::Directory(_) => direct_data.clone(),
            };
            let _ = entry.value().tx.try_send(OutgoingSseFrame { data });
        }
    }

    pub fn build_web_session_url(&self, directory: &str, session_id: &str) -> String {
        let base = self
            .config
            .web_app_url
            .clone()
            .unwrap_or_else(|| format!("http://{}", self.config.proxy_addr));
        format!(
            "{}/{}/session/{}",
            base.trim_end_matches('/'),
            encode_opencode_dir(directory),
            session_id
        )
    }
}
