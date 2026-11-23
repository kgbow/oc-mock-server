use anyhow::Result;
use clap::{Parser, Subcommand};
use oc_core::anomaly::{find_anomalies, AnomalyOptions};
use oc_core::openapi::OpenApiRegistry;
use oc_core::storage::fold_sse_events;
use oc_core::tape::{export_tape_jsonl, import_tape_jsonl};
use oc_core::*;
use oc_mcp::{mcp_http_router, run_stdio_mcp, McpBackend};
use oc_proxy::{
    build_control_router, build_proxy_router, spawn_upstream_sse_reader, AppState, ProxyConfig,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(
    name = "oc-mock-server",
    version,
    about = "OpenCode C/S debugging proxy, DevTools console, OpenAPI/LLM mock engine, Shadow Store replayer, and MCP/CLI diagnostics"
)]
struct Cli {
    /// Path to the SQLite traffic & tape database
    #[arg(long, env = "OC_MOCK_DB", default_value = "oc-mock.db", global = true)]
    db: PathBuf,

    /// Output machine-readable JSON (for scripts and AI agents)
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the dual-plane proxy (:4097) + Web DevTools / Control API / Streamable HTTP MCP (:4098)
    Serve {
        /// Proxy plane bind address (point OpenCode clients here)
        #[arg(long, env = "OC_PROXY_ADDR", default_value = "127.0.0.1:4097")]
        proxy_addr: String,

        /// Control plane & Web DevTools & MCP (/mcp) bind address
        #[arg(long, env = "OC_CONTROL_ADDR", default_value = "127.0.0.1:4098")]
        control_addr: String,

        /// Upstream OpenCode Server URL (omit to run in standalone Offline Mock/Replay mode)
        #[arg(long, env = "OC_UPSTREAM_URL")]
        upstream: Option<String>,

        /// Default workspace directory used for offline mode and directory remapping
        #[arg(long, env = "OC_DEFAULT_DIR")]
        dir: Option<String>,

        /// Optional OpenCode Web App base URL for generating direct session replay links
        #[arg(long, env = "OC_WEB_APP_URL")]
        web_app_url: Option<String>,

        /// Fetch OpenAPI /doc from upstream automatically at startup
        #[arg(long, default_value_t = true)]
        sync_openapi: bool,
    },

    /// Run an MCP JSON-RPC 2.0 server over stdio for local AI agent integration
    Mcp {
        /// Optional running Control Plane URL to forward live SSE injection & replay commands
        #[arg(long, env = "OC_CONTROL_URL")]
        control_url: Option<String>,
    },

    /// List captured sessions from traffic history
    Sessions {
        #[arg(long)]
        exclude: Option<String>,
        #[arg(long)]
        include_ignored: bool,
    },

    /// Show unified HTTP + SSE timeline for a session (folds consecutive deltas by default)
    Timeline {
        session_id: String,
        #[arg(long)]
        raw: bool,
        #[arg(long)]
        include_heartbeat: bool,
        #[arg(long, default_value_t = 200)]
        limit: usize,
    },

    /// Full-text search (FTS5) across captured HTTP & SSE traffic
    Search {
        query: String,
        #[arg(long)]
        session: Option<String>,
        #[arg(long)]
        exclude: Option<String>,
        #[arg(long)]
        kind: Option<String>,
        #[arg(long, default_value_t = 30)]
        limit: usize,
    },

    /// Automatically scan captured traffic for errors, slow calls, orphan deltas, reconnect storms, and stuck prompts
    Anomalies {
        #[arg(long)]
        session: Option<String>,
        #[arg(long)]
        exclude: Option<String>,
        #[arg(long, default_value_t = 3000)]
        slow_ms: i64,
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },

    /// Inspect full HTTP trace details by trace ID
    Http {
        trace_id: String,
        #[arg(long, default_value_t = 8000)]
        max_chars: usize,
        #[arg(long, default_value_t = 0)]
        offset: usize,
    },

    /// Inspect captured SSE events
    Sse {
        #[arg(long)]
        session: Option<String>,
        #[arg(long)]
        event_type: Option<String>,
        #[arg(long)]
        from_seq: Option<i64>,
        #[arg(long)]
        to_seq: Option<i64>,
        #[arg(long)]
        fold: bool,
        #[arg(long, default_value_t = 100)]
        limit: usize,
    },

    /// Reconstruct OpenCode client ShadowStore state snapshot at a given SSE sequence number
    State {
        session_id: String,
        #[arg(long)]
        at_seq: Option<i64>,
    },

    /// Manage replayable Session Tapes (.tape.jsonl)
    Tape {
        #[command(subcommand)]
        action: TapeAction,
    },

    /// Manage HTTP Mock / Patch / Breakpoint rules
    Rules {
        #[command(subcommand)]
        action: RuleAction,
    },
}

#[derive(Subcommand)]
enum TapeAction {
    List,
    Export {
        tape_id: String,
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    Import {
        file: PathBuf,
    },
    Extract {
        session_id: String,
        #[arg(long)]
        name: Option<String>,
    },
}

#[derive(Subcommand)]
enum RuleAction {
    List,
    Add {
        #[arg(long)]
        name: String,
        #[arg(long, default_value = "GET")]
        method: String,
        #[arg(long)]
        path: String,
        #[arg(long, default_value = "direct")]
        action_type: String,
        #[arg(long, default_value_t = 200)]
        status: u16,
        #[arg(long, default_value_t = 0)]
        delay_ms: u64,
        #[arg(long)]
        body: Option<String>,
        #[arg(long)]
        patch: Option<String>,
    },
    Delete {
        id: String,
    },
}

fn main() -> Result<()> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    match rt.block_on(async_main()) {
        Ok(()) => Ok(()),
        Err(err) => {
            if let Some(io_err) = err.downcast_ref::<std::io::Error>() {
                if io_err.kind() == std::io::ErrorKind::BrokenPipe {
                    return Ok(());
                }
            }
            Err(err)
        }
    }
}

fn outln(line: impl AsRef<str>) -> Result<()> {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    match writeln!(out, "{}", line.as_ref()) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => std::process::exit(0),
        Err(e) => Err(e.into()),
    }
}

async fn async_main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Serve {
            proxy_addr,
            control_addr,
            upstream,
            dir,
            web_app_url,
            sync_openapi,
        } => {
            init_logging(false);
            let default_directory = dir.unwrap_or_else(|| {
                std::env::current_dir()
                    .ok()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "/workspace/project".to_string())
            });
            let storage = Storage::open(&cli.db)?;
            let state = AppState::new(
                ProxyConfig {
                    proxy_addr: proxy_addr.clone(),
                    control_addr: control_addr.clone(),
                    upstream_url: upstream.clone(),
                    default_directory,
                    web_app_url,
                },
                storage,
            );

            if sync_openapi {
                if let Some(ref up) = upstream {
                    let state_sync = Arc::clone(&state);
                    let up_clone = up.clone();
                    tokio::spawn(async move {
                        match OpenApiRegistry::fetch_from_upstream(&up_clone).await {
                            Ok(reg) => {
                                tracing::info!(
                                    "synced OpenAPI spec from {}/doc ({} ops, {} events)",
                                    up_clone,
                                    reg.operations().len(),
                                    reg.event_schemas().len()
                                );
                                *state_sync.openapi.write().unwrap() = reg;
                                state_sync.broadcast_status();
                            }
                            Err(err) => {
                                tracing::warn!(
                                    "could not fetch upstream /doc (using built-in core spec): {err}"
                                );
                            }
                        }
                    });
                }
            }

            spawn_upstream_sse_reader(Arc::clone(&state));

            let proxy_app = build_proxy_router(Arc::clone(&state));
            let control_app = build_control_router(Arc::clone(&state))
                .merge(mcp_http_router(Arc::clone(&state)));

            let proxy_listener = tokio::net::TcpListener::bind(&proxy_addr).await?;
            let control_listener = tokio::net::TcpListener::bind(&control_addr).await?;

            tracing::info!("==========================================================");
            tracing::info!("oc-mock-server is running");
            tracing::info!("  Proxy Plane  : http://{proxy_addr}  (point OpenCode clients here)");
            tracing::info!("  DevTools UI  : http://{control_addr}");
            tracing::info!("  MCP Endpoint : http://{control_addr}/mcp  (Streamable HTTP)");
            tracing::info!(
                "  Upstream     : {}",
                upstream.as_deref().unwrap_or("<offline standalone mode>")
            );
            tracing::info!("  SQLite DB    : {}", cli.db.display());
            tracing::info!("==========================================================");

            tokio::try_join!(
                axum::serve(proxy_listener, proxy_app),
                axum::serve(control_listener, control_app),
            )?;
        }

        Commands::Mcp { control_url } => {
            // Send logs to stderr only so stdout stays strictly clean JSON-RPC
            init_logging(true);
            let storage = Storage::open(&cli.db)?;
            let backend = McpBackend::standalone(storage, control_url);
            run_stdio_mcp(backend).await?;
        }

        Commands::Sessions {
            exclude,
            include_ignored,
        } => {
            let storage = Storage::open(&cli.db)?;
            let list = storage.list_sessions(exclude.as_deref(), include_ignored)?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&list)?);
            } else if list.is_empty() {
                println!("No sessions captured yet.");
            } else {
                println!(
                    "{:<24} {:<6} {:<6} {:<6} {:<24} {}",
                    "SESSION_ID", "HTTP", "ERR", "SSE", "DIRECTORY", "TITLE"
                );
                for s in list {
                    println!(
                        "{:<24} {:<6} {:<6} {:<6} {:<24} {}",
                        s.session_id,
                        s.http_count,
                        s.http_error_count,
                        s.sse_count,
                        s.directory.as_deref().unwrap_or("-"),
                        s.title.as_deref().unwrap_or("-")
                    );
                }
            }
        }

        Commands::Timeline {
            session_id,
            raw,
            include_heartbeat,
            limit,
        } => {
            let storage = Storage::open(&cli.db)?;
            let items =
                storage.get_session_timeline(&session_id, !raw, include_heartbeat, limit)?;
            if cli.json {
                outln(serde_json::to_string_pretty(&items)?)?;
            } else {
                for item in items {
                    match item {
                        TimelineItem::Http { trace, .. } => {
                            outln(format!(
                                "[HTTP] {:<6} {:<38} status={} dur={}ms id={}{}",
                                trace.method,
                                trace.path,
                                trace
                                    .status
                                    .map(|s| s.to_string())
                                    .unwrap_or_else(|| "PENDING".into()),
                                trace.duration_ms.unwrap_or(0),
                                trace.id,
                                if trace.mocked { " [MOCK]" } else { "" }
                            ))?;
                        }
                        TimelineItem::Sse { event, .. } => {
                            outln(format!(
                                "[SSE ] #{:<5} {:<24} dir={} src={}",
                                event.seq,
                                event.event_type,
                                event.directory,
                                event.source.as_str()
                            ))?;
                        }
                        TimelineItem::FoldedDelta {
                            from_seq,
                            to_seq,
                            count,
                            part_id,
                            field,
                            total_chars,
                            duration_ms,
                            preview,
                            ..
                        } => {
                            outln(format!(
                                "[FOLD] #{from_seq}..#{to_seq} delta x{count} part={part_id} field={field} +{total_chars}c ({duration_ms}ms) {:?}",
                                preview
                            ))?;
                        }
                    }
                }
            }
        }

        Commands::Search {
            query,
            session,
            exclude,
            kind,
            limit,
        } => {
            let storage = Storage::open(&cli.db)?;
            let hits = storage.search_traffic(
                &query,
                session.as_deref(),
                kind.as_deref(),
                exclude.as_deref(),
                limit,
            )?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&hits)?);
            } else if hits.is_empty() {
                println!("No matches for '{query}'.");
            } else {
                for h in hits {
                    match h {
                        SearchHit::Http {
                            id,
                            method,
                            path,
                            status,
                            snippet,
                            ..
                        } => {
                            println!(
                                "[HTTP] {id} {method} {path} ({:?}) :: {snippet}",
                                status
                            );
                        }
                        SearchHit::Sse {
                            seq,
                            event_type,
                            snippet,
                            ..
                        } => {
                            println!("[SSE ] #{seq} {event_type} :: {snippet}");
                        }
                    }
                }
            }
        }

        Commands::Anomalies {
            session,
            exclude,
            slow_ms,
            limit,
        } => {
            let storage = Storage::open(&cli.db)?;
            let list = find_anomalies(
                &storage,
                &AnomalyOptions {
                    session_id: session,
                    exclude_session_id: exclude,
                    slow_http_ms: slow_ms,
                    limit,
                },
            )?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&list)?);
            } else if list.is_empty() {
                println!("No anomalies detected.");
            } else {
                for a in list {
                    println!(
                        "[{:?}] {} ({}) — {}",
                        a.severity, a.kind, a.ref_id, a.message
                    );
                }
            }
        }

        Commands::Http {
            trace_id,
            max_chars,
            offset,
        } => {
            let storage = Storage::open(&cli.db)?;
            let mut trace = storage
                .get_http_trace(&trace_id)?
                .ok_or_else(|| anyhow::anyhow!("trace '{trace_id}' not found"))?;
            if let Some(ref mut rb) = trace.res_body {
                *rb = rb.chars().skip(offset).take(max_chars).collect();
            }
            println!("{}", serde_json::to_string_pretty(&trace)?);
        }

        Commands::Sse {
            session,
            event_type,
            from_seq,
            to_seq,
            fold,
            limit,
        } => {
            let storage = Storage::open(&cli.db)?;
            let events = storage.list_sse_events(
                session.as_deref(),
                event_type.as_deref(),
                None,
                from_seq,
                to_seq,
                true,
                None,
                limit,
                false,
            )?;
            if fold {
                let folded = fold_sse_events(events);
                println!("{}", serde_json::to_string_pretty(&folded)?);
            } else {
                println!("{}", serde_json::to_string_pretty(&events)?);
            }
        }

        Commands::State { session_id, at_seq } => {
            let storage = Storage::open(&cli.db)?;
            let events = storage.list_sse_events(
                Some(&session_id),
                None,
                None,
                None,
                at_seq,
                true,
                None,
                5000,
                false,
            )?;
            let store = ShadowStore::reconstruct_from_records(&events, at_seq);
            println!(
                "{}",
                serde_json::to_string_pretty(&store.snapshot_json(Some(&session_id)))?
            );
        }

        Commands::Tape { action } => {
            let storage = Storage::open(&cli.db)?;
            match action {
                TapeAction::List => {
                    let list = storage.list_tapes()?;
                    if cli.json {
                        println!("{}", serde_json::to_string_pretty(&list)?);
                    } else {
                        for t in list {
                            println!(
                                "{:<22} events={:<4} dur={}ms  {}",
                                t.id, t.event_count, t.duration_ms, t.name
                            );
                        }
                    }
                }
                TapeAction::Export { tape_id, output } => {
                    let tape = storage
                        .get_tape(&tape_id)?
                        .ok_or_else(|| anyhow::anyhow!("tape '{tape_id}' not found"))?;
                    let jsonl = export_tape_jsonl(&tape)?;
                    if let Some(out_path) = output {
                        std::fs::write(&out_path, jsonl)?;
                        println!("Exported {} to {}", tape_id, out_path.display());
                    } else {
                        print!("{jsonl}");
                    }
                }
                TapeAction::Import { file } => {
                    let raw = std::fs::read_to_string(&file)?;
                    let tape = import_tape_jsonl(&raw)?;
                    storage.save_tape(&tape)?;
                    if cli.json {
                        println!("{}", serde_json::to_string_pretty(&tape)?);
                    } else {
                        println!("Imported tape {} ({} events)", tape.id, tape.events.len());
                    }
                }
                TapeAction::Extract { session_id, name } => {
                    let tape = storage.extract_tape_from_session(&session_id, name)?;
                    if cli.json {
                        println!("{}", serde_json::to_string_pretty(&tape)?);
                    } else {
                        println!(
                            "Extracted tape {} ({} events) from session {}",
                            tape.id,
                            tape.events.len(),
                            session_id
                        );
                    }
                }
            }
        }

        Commands::Rules { action } => {
            let storage = Storage::open(&cli.db)?;
            match action {
                RuleAction::List => {
                    let rules = storage.list_mock_rules()?;
                    if cli.json {
                        println!("{}", serde_json::to_string_pretty(&rules)?);
                    } else {
                        for r in rules {
                            println!(
                                "{:<16} [{}] {:<10} {:<6} {:<30} status={}",
                                r.id,
                                if r.enabled { "ON " } else { "OFF" },
                                r.action_type.as_str(),
                                r.method,
                                r.path_pattern,
                                r.status
                            );
                        }
                    }
                }
                RuleAction::Add {
                    name,
                    method,
                    path,
                    action_type,
                    status,
                    delay_ms,
                    body,
                    patch,
                } => {
                    let parsed_body: Option<Value> = body
                        .map(|s| serde_json::from_str(&s).unwrap_or(Value::String(s)));
                    let parsed_patch: Option<Value> =
                        patch.map(|s| serde_json::from_str(&s)).transpose()?;
                    let rule = MockRule {
                        id: format!("rule_{}", &uuid::Uuid::new_v4().simple().to_string()[..10]),
                        name,
                        enabled: true,
                        priority: 50,
                        method: method.to_uppercase(),
                        path_pattern: path,
                        directory_filter: None,
                        action_type: MockActionType::parse(&action_type),
                        status,
                        delay_ms,
                        headers: BTreeMap::new(),
                        body: parsed_body,
                        patch: parsed_patch,
                        created_at: now_ms(),
                    };
                    storage.upsert_mock_rule(&rule)?;
                    if cli.json {
                        println!("{}", serde_json::to_string_pretty(&rule)?);
                    } else {
                        println!("Created mock rule {}", rule.id);
                    }
                }
                RuleAction::Delete { id } => {
                    let deleted = storage.delete_mock_rule(&id)?;
                    if cli.json {
                        println!("{}", json!({ "deleted": deleted }));
                    } else {
                        println!("Deleted {}: {}", id, deleted);
                    }
                }
            }
        }
    }

    Ok(())
}

fn init_logging(to_stderr: bool) {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,oc_proxy=info,oc_mcp=info"));
    if to_stderr {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(std::io::stderr)
            .init();
    } else {
        tracing_subscriber::fmt().with_env_filter(filter).init();
    }
}
