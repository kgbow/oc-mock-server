# `oc-mock-server` Architecture & Protocol Design

## 1. Overview

`oc-mock-server` is a Rust + Web debugging proxy, DevTools inspector, OpenAPI/LLM mock engine, Shadow Store session replayer, and MCP/CLI traffic diagnostics tool designed specifically for **OpenCode's Client/Server architecture**.

It sits transparently between OpenCode clients (Web App, Desktop App, or `opencode attach <url>`) and an OpenCode server, or runs completely **offline** in standalone Mock/Replay mode.

---

## 2. Dual-Plane Architecture

A single Rust process binds two separate ports to eliminate route, CORS, and recursion conflicts:

1. **Proxy Plane (default `127.0.0.1:4097`)** — Presents an OpenCode-compatible server surface to clients:
   - **HTTP Reverse Proxy**: Streaming request/response passthrough with two-phase trace reporting (`Pending` -> `Completed`), CORS/Basic Auth forwarding, body tee capture (bounded), and `directory` / `workspace` / `sessionID` semantic extraction.
   - **Mock, Patch & Breakpoint Engine**: Evaluates priority-ordered rules matching `method`, `path_pattern` (supporting `:param` and `*`), and optional `directory_filter`:
     - `direct`: Immediate local response with optional artificial latency (`delay_ms`).
     - `patch`: Forward to upstream, then merge/patch JSON response before returning.
     - `breakpoint`: Suspend in-flight request on a `tokio::sync::oneshot` channel; Web DevTools inspects/edits status, headers, or body and clicks Resume / Mock.
   - **Decoupled SSE Hub**:
     - Compatible with both `GET /global/event` (`{ directory, payload }`) and `GET /event` (`payload` filtered by `directory`).
     - Decouples upstream ingestion from downstream fan-out so operators can switch between `passthrough`, `intercept` (mute upstream / filter), and `replay` modes on the fly.
     - Injects an **independent `server.heartbeat` every 9 seconds** per client connection to prevent OpenCode's 15-second client heartbeat timeout from disconnecting and triggering a full bootstrap reload during pause/step debugging.
   - **PTY WebSocket Bridge**: Transparent bidirectional bridge for `GET /pty/:ptyID/connect`.
   - **Shadow Store Virtual Endpoints**: During Replay or Offline mode, intercepts/merges `GET /session`, `GET /session/:id`, `GET /session/:id/message`, `GET /session/status`, `GET /session/:id/todo`, `GET /session/:id/diff`, `GET /permission`, `GET /question`, and offline bootstrap routes (`/global/health`, `/path`, `/project`, `/project/current`, `/config`, `/global/config`, `/provider`, `/agent`, `/command`, `/vcs`, `/mcp`, `/lsp`, `/formatter`).

2. **Control & Diagnostics Plane (default `127.0.0.1:4098`)**:
   - Hosts the embedded Web DevTools SPA (`rust-embed` from `web/dist`).
   - Exposes REST Control API (`/api/*`) for traffic queries, rules, breakpoints, OpenAPI sync, Faker/LLM generation, Tape recording/import/export, and Replay control.
   - Exposes Live DevTools WebSocket (`/api/ws`) pushing HTTP trace updates, SSE frames, breakpoint notifications, and replay cursor progress in real time.
   - Exposes **MCP Streamable HTTP endpoint (`/mcp`)** implementing MCP JSON-RPC 2.0 (`POST /mcp`, `GET /mcp`, `DELETE /mcp` with `Mcp-Session-Id`) so OpenCode agents can connect directly via `opencode.json`.

---

## 3. OpenCode-Specific Replay Guarantees

To let an **unmodified** OpenCode client faithfully replay recorded or AI-generated session details:

1. **Session Tape = Bootstrap Snapshot + Timed SSE Stream (`.tape.jsonl`)**:
   - Captures initial HTTP state alongside ordered SSE events with `offset_ms`.
2. **Rust Shadow Store**:
   - Mirrors OpenCode's frontend event reducer (`session.*`, `message.*`, `message.part.updated`, `message.part.delta`, `message.part.removed`, `todo.updated`, `permission.*`, `question.*`).
   - Advances incrementally with the replay cursor (and rebuilds deterministically on Seek).
   - Serves virtual session state to `GET /session*` requests so opening a session or refreshing never hits 404 or stale upstream data.
   - Supports **Hybrid Mount** (injects the virtual replay session alongside real upstream sessions) and **Offline Mode** (serves all bootstrap routes locally with no upstream required).
3. **Automatic Directory Remap**:
   - Tracks active client directories from incoming requests (`?directory=` / `x-opencode-directory`) and rewrites `directory` in replayed events so OpenCode's `children.children[directory]` guard never drops events silently.
4. **Auto-Navigation Helpers**:
   - **TUI**: Injects native `tui.session.select` SSE event (`{ sessionID }`).
   - **Web App**: Generates direct route `/${base64(directory)}/session/${sessionID}` and supports side-by-side preview.

---

## 4. OpenAPI `/doc` + LLM Mock & Scenario Pipeline

OpenCode exposes both REST schemas and the complete `BusEvent.payloads()` discriminated union under `GET /doc`.

- **Schema Sync**: Fetches `/doc` from upstream (falling back to an embedded OpenCode core OpenAPI spec offline).
- **Deterministic Faker**: Resolves `$ref`, `oneOf`/`anyOf`/`allOf`, `enum`, and OpenCode ID conventions (`ses_*`, `msg_*`, `prt_*`) to generate valid JSON instantly.
- **LLM Scenario Generator & Chaos Mutator**:
  - Extracts a compact sub-schema for the requested route or event types.
  - Calls any OpenAI-compatible Chat Completions endpoint (`api_base`, `api_key`, `model`).
  - Validates output against JSON Schema (`jsonschema`) and semantic sequence invariants (e.g., `message.part.updated` precedes `message.part.delta`, matching IDs, monotonic timestamps), running an automatic repair prompt on validation failures.

---

## 5. MCP & CLI Diagnostics Design

Both `oc-mock-server mcp` (stdio), `:4098/mcp` (Streamable HTTP), and `oc-mock-server <subcommand> --json` share `oc-core`'s query & analysis engine over SQLite WAL + FTS5:

- **Server-Side Delta Folding**: Consecutive `message.part.delta` events for the same `(messageID, partID, field)` fold into a single summary item (`count`, `chars`, `duration_ms`, `preview`) by default, cutting AI agent context usage by >95%.
- **Automatic Anomaly Detection (`find_anomalies`)**:
  - HTTP 4xx/5xx responses and upstream network errors
  - Slow requests above threshold
  - Reconnect storms (repeated `server.connected` events)
  - Orphan `message.part.delta` events missing a prior `message.part.updated`
  - Unanswered `permission.asked` / `question.asked` prompts
  - Directory mismatches across a session
- **Point-in-Time State Reconstruction (`reconstruct_state_at`)**: Runs the Shadow Store up to any `seq` and returns the exact `Session / Message / Part / Status / Todo / Permission / Question` snapshot.
- **Self-Traffic Exclusion**: Supports `exclude_session_id` and per-session ignore flags so debugging an OpenCode session with an agent doesn't pollute diagnostics.
