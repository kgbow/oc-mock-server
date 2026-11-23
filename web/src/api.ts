import type {
  Anomaly,
  HttpTrace,
  HttpTraceSummary,
  LlmConfigView,
  MockRule,
  OpenApiSummary,
  PendingBreakpoint,
  ProxyMode,
  ProxyRuntimeStatus,
  ReplayStatus,
  SessionOverview,
  SseRecord,
  SseTimelineEntry,
  TapeSummary,
} from './types'

async function req<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(path, {
    ...init,
    headers: {
      'content-type': 'application/json',
      ...(init?.headers ?? {}),
    },
  })
  const text = await res.text()
  let parsed: unknown = null
  try {
    parsed = text ? JSON.parse(text) : null
  } catch {
    parsed = text
  }
  if (!res.ok) {
    const msg =
      typeof parsed === 'object' && parsed !== null && 'error' in parsed
        ? String((parsed as { error: unknown }).error)
        : `HTTP ${res.status}`
    throw new Error(msg)
  }
  return parsed as T
}

export const api = {
  getStatus: () => req<ProxyRuntimeStatus>('/api/status'),
  setMode: (mode: ProxyMode) =>
    req<ProxyRuntimeStatus>('/api/mode', { method: 'POST', body: JSON.stringify({ mode }) }),
  setMuteSse: (enabled: boolean) =>
    req<ProxyRuntimeStatus>('/api/mute-sse', { method: 'POST', body: JSON.stringify({ enabled }) }),
  setRecording: (enabled: boolean) =>
    req<ProxyRuntimeStatus>('/api/recording', { method: 'POST', body: JSON.stringify({ enabled }) }),
  clearTraffic: () => req<{ cleared: boolean }>('/api/traffic', { method: 'DELETE' }),

  listHttp: (params?: { session_id?: string; method?: string; limit?: number }) => {
    const q = new URLSearchParams()
    if (params?.session_id) q.set('session_id', params.session_id)
    if (params?.method) q.set('method', params.method)
    if (params?.limit) q.set('limit', String(params.limit))
    return req<HttpTraceSummary[]>(`/api/http?${q.toString()}`)
  },
  getHttp: (id: string) => req<HttpTrace>(`/api/http/${encodeURIComponent(id)}`),

  listSse: (params?: { session_id?: string; event_type?: string; fold?: boolean; limit?: number }) => {
    const q = new URLSearchParams()
    if (params?.session_id) q.set('session_id', params.session_id)
    if (params?.event_type) q.set('event_type', params.event_type)
    if (params?.fold) q.set('fold', 'true')
    if (params?.limit) q.set('limit', String(params.limit))
    return req<SseRecord[] | SseTimelineEntry[]>(`/api/sse?${q.toString()}`)
  },
  injectSse: (body: { directory?: string; payload: unknown; apply_to_shadow?: boolean }) =>
    req<{ ok: boolean; recorded: SseRecord | null }>('/api/sse/inject', {
      method: 'POST',
      body: JSON.stringify(body),
    }),

  listSessions: () => req<SessionOverview[]>('/api/sessions'),
  getSessionState: (sessionId: string, atSeq?: number) => {
    const q = atSeq !== undefined ? `?at_seq=${atSeq}` : ''
    return req<unknown>(`/api/sessions/${encodeURIComponent(sessionId)}/state${q}`)
  },
  listAnomalies: (sessionId?: string) => {
    const q = sessionId ? `?session_id=${encodeURIComponent(sessionId)}` : ''
    return req<Anomaly[]>(`/api/anomalies${q}`)
  },

  listRules: () => req<MockRule[]>('/api/rules'),
  createRule: (rule: {
    name: string
    enabled?: boolean
    priority?: number
    method: string
    path_pattern: string
    directory_filter?: string | null
    action_type: 'direct' | 'patch' | 'breakpoint'
    status?: number
    delay_ms?: number
    headers?: Record<string, string>
    body?: unknown
    patch?: unknown
  }) => req<MockRule>('/api/rules', { method: 'POST', body: JSON.stringify(rule) }),
  updateRule: (id: string, rule: Omit<MockRule, 'id' | 'created_at'>) =>
    req<MockRule>(`/api/rules/${encodeURIComponent(id)}`, {
      method: 'PUT',
      body: JSON.stringify(rule),
    }),
  deleteRule: (id: string) =>
    req<{ deleted: boolean }>(`/api/rules/${encodeURIComponent(id)}`, { method: 'DELETE' }),

  listBreakpoints: () => req<PendingBreakpoint[]>('/api/breakpoints'),
  resolveBreakpoint: (
    id: string,
    decision:
      | { decision: 'forward'; req_body?: string | null; req_headers?: Record<string, string> | null }
      | { decision: 'respond'; status: number; headers: Record<string, string>; body: string }
      | { decision: 'abort'; message?: string },
  ) =>
    req<{ resolved: boolean }>(`/api/breakpoints/${encodeURIComponent(id)}/resolve`, {
      method: 'POST',
      body: JSON.stringify(decision),
    }),

  listTapes: () => req<TapeSummary[]>('/api/tapes'),
  extractTape: (sessionId: string, name?: string) =>
    req<TapeSummary>(`/api/tapes/extract/${encodeURIComponent(sessionId)}`, {
      method: 'POST',
      body: JSON.stringify({ name }),
    }),
  importTape: (jsonl: string) =>
    req<TapeSummary>('/api/tapes/import', { method: 'POST', body: JSON.stringify({ jsonl }) }),
  exportTape: async (id: string) => {
    const res = await fetch(`/api/tapes/${encodeURIComponent(id)}/export`)
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
    return res.text()
  },
  deleteTape: (id: string) =>
    req<{ deleted: boolean }>(`/api/tapes/${encodeURIComponent(id)}`, { method: 'DELETE' }),

  replayLoad: (body: {
    tape_id: string
    target_directory?: string
    mount_shadow?: boolean
    speed?: number
    min_frame_ms?: number
  }) => req<ReplayStatus>('/api/replay/load', { method: 'POST', body: JSON.stringify(body) }),
  replayPlay: () => req<ReplayStatus>('/api/replay/play', { method: 'POST' }),
  replayPause: () => req<ReplayStatus>('/api/replay/pause', { method: 'POST' }),
  replayStop: () => req<ReplayStatus>('/api/replay/stop', { method: 'POST' }),
  replayStep: () => req<ReplayStatus>('/api/replay/step', { method: 'POST' }),
  replaySeek: (cursor: number, notify_clients = true) =>
    req<ReplayStatus>('/api/replay/seek', {
      method: 'POST',
      body: JSON.stringify({ cursor, notify_clients }),
    }),
  replaySpeed: (speed: number, min_frame_ms?: number) =>
    req<ReplayStatus>('/api/replay/speed', {
      method: 'POST',
      body: JSON.stringify({ speed, min_frame_ms }),
    }),
  replayTuiSelect: (session_id?: string) =>
    req<{ ok: boolean }>('/api/replay/tui-select', {
      method: 'POST',
      body: JSON.stringify({ session_id }),
    }),

  getOpenApi: () => req<OpenApiSummary>('/api/openapi'),
  syncOpenApi: (url?: string) =>
    req<OpenApiSummary>('/api/openapi/sync', { method: 'POST', body: JSON.stringify({ url }) }),
  fakeHttp: (method: string, path: string, directory?: string) =>
    req<unknown>('/api/openapi/fake-http', {
      method: 'POST',
      body: JSON.stringify({ method, path, directory }),
    }),
  fakeSse: (event_type: string, directory?: string) =>
    req<unknown>('/api/openapi/fake-sse', {
      method: 'POST',
      body: JSON.stringify({ event_type, directory }),
    }),

  getLlmConfig: () => req<LlmConfigView>('/api/llm/config'),
  setLlmConfig: (cfg: { api_base: string; api_key: string; model: string; temperature: number }) =>
    req<{ ok: boolean }>('/api/llm/config', { method: 'POST', body: JSON.stringify(cfg) }),
  llmGenerateHttp: (body: { method: string; path: string; prompt: string; directory?: string }) =>
    req<{ data: unknown; validated: boolean; attempts: number; warnings: string[] }>(
      '/api/llm/generate-http',
      { method: 'POST', body: JSON.stringify(body) },
    ),
  llmGenerateScenario: (body: {
    prompt: string
    directory?: string
    tape_name?: string
    save_tape?: boolean
  }) =>
    req<{ tape: TapeSummary; validated: boolean; attempts: number; warnings: string[] }>(
      '/api/llm/generate-scenario',
      { method: 'POST', body: JSON.stringify(body) },
    ),
  llmMutate: (body: { payload: unknown; instruction: string }) =>
    req<{ data: unknown }>('/api/llm/mutate', { method: 'POST', body: JSON.stringify(body) }),
}
