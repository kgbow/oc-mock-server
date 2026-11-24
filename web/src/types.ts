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
  mocked?: boolean
  mock_rule_id?: string | null
  dropped?: boolean
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

export type MockActionType = 'direct' | 'patch' | 'breakpoint' | 'sse_stream'

export type BreakpointPhase = 'request' | 'response' | 'both'

export interface MockSseStep {
  delay_ms: number
  payload: unknown
  directory?: string | null
  event?: string | null
  id?: string | null
  apply_to_shadow?: boolean
}

export interface MockRule {
  id: string
  name: string
  enabled: boolean
  priority: number
  method: string
  path_pattern: string
  query_pattern: string | null
  body_contains: string | null
  directory_filter: string | null
  action_type: MockActionType
  breakpoint_phase: BreakpointPhase
  one_shot: boolean
  hit_count: number
  status: number
  delay_ms: number
  headers: Record<string, string>
  body: unknown | null
  patch: unknown | null
  sse_sequence: MockSseStep[]
  created_at: number
}

export interface CreateMockRuleInput {
  name: string
  enabled?: boolean
  priority?: number
  method: string
  path_pattern: string
  query_pattern?: string | null
  body_contains?: string | null
  directory_filter?: string | null
  action_type: MockActionType
  breakpoint_phase?: BreakpointPhase
  one_shot?: boolean
  status?: number
  delay_ms?: number
  headers?: Record<string, string>
  body?: unknown
  patch?: unknown
  sse_sequence?: MockSseStep[]
}

export type SseMockActionType = 'drop' | 'patch' | 'replace' | 'delay' | 'breakpoint'

export interface SseMockRule {
  id: string
  name: string
  enabled: boolean
  priority: number
  event_type_pattern: string
  session_filter: string | null
  directory_filter: string | null
  payload_contains: string | null
  action_type: SseMockActionType
  delay_ms: number
  patch: unknown | null
  replace_events: MockSseStep[]
  one_shot: boolean
  hit_count: number
  created_at: number
}

export interface CreateSseMockRuleInput {
  name: string
  enabled?: boolean
  priority?: number
  event_type_pattern: string
  session_filter?: string | null
  directory_filter?: string | null
  payload_contains?: string | null
  action_type: SseMockActionType
  delay_ms?: number
  patch?: unknown
  replace_events?: MockSseStep[]
  one_shot?: boolean
}

export type BreakpointKind = 'http_request' | 'http_response' | 'sse_event'

export interface PendingBreakpoint {
  id: string
  kind: BreakpointKind
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
  res_status: number | null
  res_headers: Record<string, string>
  res_body: string | null
  sse_event_type: string | null
  sse_session_id: string | null
  sse_payload: unknown | null
}

export interface TapeSnapshot {
  projects?: unknown[]
  sessions?: unknown[]
  messages?: Record<string, unknown[]>
  parts?: Record<string, unknown[]>
  statuses?: Record<string, unknown>
  todos?: Record<string, unknown[]>
  diffs?: Record<string, unknown[]>
}

export interface TapeEvent {
  offset_ms: number
  directory: string
  payload: unknown
}

export interface Tape {
  id: string
  name: string
  description: string | null
  directory: string
  session_id: string | null
  snapshot: TapeSnapshot
  events: TapeEvent[]
  created_at: number
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
  | { type: 'sse_rules_updated' }
  | { type: 'tapes_updated' }
