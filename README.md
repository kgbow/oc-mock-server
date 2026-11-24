# oc-mock-server

> Rust + Web debugging proxy, browser-DevTools-style HTTP/SSE inspector, OpenAPI `/doc` + LLM mock studio, Shadow Store session replayer, and MCP/CLI traffic diagnostics server for **OpenCode** C/S architectures.

## Highlights

- **Zero-Intrusion Dual-Plane Proxy** (`:4097` Proxy Plane + `:4098` Control Plane):
  - Transparent HTTP streaming proxy with two-phase trace reporting (`Pending` -> `Completed`), CORS/Auth passthrough, and `directory` / `sessionID` semantic indexing.
  - **Decoupled SSE Hub** supporting both `GET /global/event` (Web/Desktop App) and `GET /event` (TUI `opencode attach`), with **independent 9s `server.heartbeat`** keepalive so paused/stepped replays never trip OpenCode's 15s client watchdog.
  - **PTY WebSocket Bridge** for `GET /pty/:ptyID/connect`.
- **Browser DevTools-Style Embedded Web Console** (single Rust binary via `rust-embed`):
  - **Network**: Live HTTP table, JSON formatter, headers inspector, cURL copy, full manual **Edit & Mock** studio (status, headers, JSON body, `:param` / `*` / `regex:` matchers, HTTP-triggered SSE sequences), and AI Chaos Mutation directly into the manual editor.
  - **SSE Stream**: Live timeline with **Delta Folding**, filtering, mute upstream toggle, **Manual Edit & Re-emit** on any captured frame, and dynamic **SSE Stream Rules** (`drop`, `patch`, `replace`, `delay`, `breakpoint`).
  - **Mock & Dynamic Breakpoints**: Priority-ordered HTTP (`direct`, `patch`, `sse_stream`, `breakpoint`) and SSE (`drop`, `patch`, `replace`, `delay`, `breakpoint`) rules with full manual editing, `one_shot` auto-disable, live **Request / Response / SSE Frame Breakpoints**, and real `Passthrough` (bypass all rules) vs `Intercept` mode separation.
  - **OpenAPI `/doc` + LLM Scenario Studio**: Syncs OpenCode's live `/doc` (both REST schemas and `BusEvent.payloads()`), runs deterministic Schema Faker offline, and generates editable schema-validated HTTP mocks or full multi-frame SSE Replay Tapes via any OpenAI-compatible LLM.
  - **Replay, Tape Editor & Shadow Store**: Portable `.tape.jsonl` recording/import/export, visual **Tape SSE Frame Editor** (insert, reorder, duplicate, delete, and manually edit individual event frames & offsets), automatic **Directory Remap**, hybrid virtual session mounting into `GET /session*`, Play/Pause/Step/Speed/Seek controls, `tui.session.select` auto-navigation for TUI, and direct session URLs for Web App.
- **MCP + CLI Diagnostics for AI Agents**:
  - **Streamable HTTP MCP** built into `http://127.0.0.1:4098/mcp` + **stdio MCP** (`oc-mock-server mcp`).
  - **Server-Side Delta Folding** reduces AI agent token consumption by >95%.
  - **Automated Anomaly Detection (`find_anomalies`)** detects HTTP 4xx/5xx, slow requests, reconnect storms, orphan `message.part.delta` frames, directory mismatches, and unanswered `permission.asked` / `question.asked` prompts.
  - **Point-in-Time ShadowState Reconstruction (`reconstruct_state_at`)** rebuilds exact client store state at any SSE `seq`.

---

## Quick Start

### 1. Build Web UI + Single Rust Binary

```bash
cd web && npm install && npm run build && cd ..
cargo build --release
```

### 2. Run Against an OpenCode Server (or Standalone Offline)

```bash
# Proxy mode (forwards to OpenCode server on :4096)
./target/release/oc-mock-server serve \
  --upstream http://127.0.0.1:4096 \
  --proxy-addr 127.0.0.1:4097 \
  --control-addr 127.0.0.1:4098

# Standalone offline Replay / Mock mode (no real OpenCode server needed)
./target/release/oc-mock-server serve --dir /path/to/your/project
```

Then:
- Open the Web DevTools Console: **http://127.0.0.1:4098**
- Attach an OpenCode TUI client to the proxy:
  ```bash
  opencode attach http://127.0.0.1:4097
  ```
- Or configure OpenCode Web / Desktop App server URL to `http://127.0.0.1:4097`.

---

## Connecting AI Agents via MCP

### Remote Streamable HTTP (Recommended — live proxy & write tools)

Add to your `opencode.json`:

```json
{
  "mcp": {
    "oc-mock-server": {
      "type": "remote",
      "url": "http://127.0.0.1:4098/mcp",
      "oauth": false
    }
  }
}
```

### Local Stdio MCP

```json
{
  "mcp": {
    "oc-mock-server": {
      "type": "local",
      "command": [
        "oc-mock-server",
        "mcp",
        "--db",
        "oc-mock.db",
        "--control-url",
        "http://127.0.0.1:4098"
      ]
    }
  }
}
```

### Exposed MCP Tools
| Tool | Description |
| :--- | :--- |
| `list_sessions` | List captured sessions with HTTP/SSE/error counts (`exclude_session_id` supported) |
| `get_session_timeline` | Chronological HTTP + folded SSE timeline (saves >95% context tokens) |
| `search_traffic` | SQLite FTS5 full-text search across HTTP URLs/bodies and SSE payloads |
| `find_anomalies` | Automatic scanner for 4xx/5xx, slow calls, orphan deltas, reconnect storms, stuck prompts |
| `get_http_detail` | Full HTTP headers & paginated body inspection (`max_body_chars` + `body_offset`) |
| `get_sse_range` | Inspect SSE events by sequence range (`fold_deltas` toggle) |
| `reconstruct_state_at` | Rebuild client ShadowStore snapshot (`Session/Messages/Parts/Todos/Diffs`) at any `seq` |
| `create_mock_rule` | Dynamically create HTTP `direct` / `patch` / `sse_stream` / `breakpoint` (`request`/`response`/`both`) rules with optional triggered SSE sequences |
| `create_sse_mock_rule` | Dynamically create live SSE stream `drop` / `patch` / `replace` / `delay` / `breakpoint` rules |
| `inject_sse_event` | Push synthetic OpenCode SSE events directly to connected clients |
| `control_replay` | Load, play, pause, stop, step, seek, or trigger `tui.session.select` |

---

## CLI Usage

Every subcommand supports `--json` and `--db <path>`:

```bash
oc-mock-server sessions --json
oc-mock-server timeline ses_xxx
oc-mock-server timeline ses_xxx --raw
oc-mock-server search "UnknownError" --json
oc-mock-server anomalies --slow-ms 2000 --json
oc-mock-server http tr_xxxxxxxxxxxx
oc-mock-server state ses_xxx --at-seq 42
oc-mock-server tape list
oc-mock-server tape extract ses_xxx --name "Bug repro"
oc-mock-server tape export tape_demo_heartbeat -o demo.tape.jsonl
oc-mock-server tape import demo.tape.jsonl
oc-mock-server rules add --name "Fail VCS" --method GET --path /vcs --status 500 --body '{"error":"boom"}'
oc-mock-server rules add --name "Edit Health Resp" --method GET --path /global/health --action-type breakpoint --phase response --one-shot
oc-mock-server sse-rules add --name "Drop Idle" --event-type session.idle --action-type drop --one-shot
```

---

## LLM Configuration (Optional)

Set via the Web UI (**MCP & Config**) or environment variables:

```bash
export OC_LLM_API_BASE="https://api.openai.com/v1" # or Ollama http://127.0.0.1:11434/v1
export OC_LLM_API_KEY="sk-..."
export OC_LLM_MODEL="gpt-4o-mini"
```

---

## Architecture & Design

See [docs/DESIGN.md](docs/DESIGN.md) for the complete architecture, Shadow Store reducer semantics, directory remapping protocol, and SQLite FTS5 storage layout.
