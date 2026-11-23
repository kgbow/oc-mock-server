export type ProxyMode = 'passthrough' | 'intercept' | 'replay'

export type ReplayStateKind = 'stopped' | 'playing' | 'paused' | 'completed'

export interface ReplayStatus {
  state: ReplayStateKind
  tape_id: string | null
  tape_name: string | null
  session_id: string | null
  cursor: number
  total_events: number
  speed: number
  min_frame_ms: number
  target_directory: string | null
  mount_shadow: boolean
  web_session_url: string | null
}

export interface ProxyRuntimeStatus {
  proxy_addr: string
  control_addr: string
  upstream_url: string | null
  mode: ProxyMode
  mute_upstream_sse: boolean
  recording: boolean
  active_directories: string[]
  connected_sse_clients: number
  pending_breakpoints: number
  openapi_source: string
  replay: ReplayStatus
}

export interface HttpTraceSummary {
  id: string
  started_at: number
  duration_ms: number | null
  method: string
  path: string
  query: string | null
  directory: string | null
  session_id: string | null
  status: number | null
  req_size: number
  res_size: number
  mocked: boolean
  mock_rule_id: string | null
  upstream_error: string | null
  pending: boolean
}

export interface HttpTrace extends HttpTraceSummary {
  completed_at: number | null
  req_headers: Record<string, string>
  req_body: string | null
  res_headers: Record<string, string>
  res_body: string | null
}

export interface SseRecord {
  seq: number
  ts: number
  source: 'upstream' | 'injected' | 'replay'
  directory: string
  event_type: string
  session_id: string | null
  message_id: string | null
  part_id: string | null
  payload: unknown
}

export interface FoldedDeltaItem {
  kind: 'folded_delta'
  ts: number
  end_ts: number
  from_seq: number
  to_seq: number
  count: number
  directory: string
  session_id: string | null
  message_id: string
  part_id: string
  field: string
  total_chars: number
  duration_ms: number
  preview: string
}

export type SseTimelineEntry =
  | { kind: 'sse'; ts: number; event: SseRecord }
  | FoldedDeltaItem

export interface SessionOverview {
  session_id: string
  directory: string | null
  title: string | null
  first_ts: number
  last_ts: number
  http_count: number
  http_error_count: number
  sse_count: number
  ignored: boolean
}

export interface MockRule {
  id: string
  name: string
  enabled: boolean
  priority: number
  method: string
  path_pattern: string
  directory_filter: string | null
  action_type: 'direct' | 'patch' | 'breakpoint'
  status: number
  delay_ms: number
  headers: Record<string, string>
  body: unknown | null
  patch: unknown | null
  created_at: number
}

export interface PendingBreakpoint {
  id: string
  trace_id: string
  rule_id: string
  rule_name: string
  started_at: number
  method: string
  path: string
  query: string | null
  directory: string | null
  req_headers: Record<string, string>
  req_body: string | null
}

export interface TapeSummary {
  id: string
  name: string
  description: string | null
  directory: string
  session_id: string | null
  event_count: number
  duration_ms: number
  created_at: number
}

export interface Anomaly {
  severity: 'error' | 'warning' | 'info'
  kind: string
  ts: number
  session_id: string | null
  ref_id: string
  message: string
  details: unknown
}

export interface OpenApiOperation {
  method: string
  path: string
  operation_id: string | null
  summary: string | null
  description: string | null
  response_schema: unknown | null
  request_schema: unknown | null
}

export interface OpenApiEventSchema {
  event_type: string
  schema: unknown
}

export interface OpenApiSummary {
  source: string
  title: string
  version: string
  operations: OpenApiOperation[]
  events: OpenApiEventSchema[]
}

export interface LlmConfigView {
  configured: boolean
  api_base?: string
  model?: string
  temperature?: number
  has_api_key?: boolean
}

export type ControlWsEvent =
  | { type: 'status'; status: ProxyRuntimeStatus }
  | { type: 'http_started'; trace: HttpTraceSummary }
  | { type: 'http_completed'; trace: HttpTraceSummary }
  | { type: 'sse_recorded'; event: SseRecord }
  | { type: 'breakpoint_added'; breakpoint: PendingBreakpoint }
  | { type: 'breakpoint_resolved'; id: string }
  | { type: 'replay_updated'; replay: ReplayStatus }
  | { type: 'rules_updated' }
  | { type: 'tapes_updated' }
