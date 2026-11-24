import { useState } from 'react'
import { Copy, Edit3, Plus, RefreshCw, Save, Sparkles, Trash2, Wand2, X } from 'lucide-react'
import { api } from '../api'
import type {
  BreakpointPhase,
  MockActionType,
  MockRule,
  MockSseStep,
  OpenApiSummary,
  SseMockActionType,
  SseMockRule,
} from '../types'

interface StepDraft {
  delay_ms: number
  payloadText: string
  directory: string
  event: string
  apply_to_shadow: boolean
}

function toStepDrafts(steps: MockSseStep[] | undefined): StepDraft[] {
  return (steps ?? []).map((s) => ({
    delay_ms: s.delay_ms ?? 0,
    payloadText:
      typeof s.payload === 'string' ? s.payload : JSON.stringify(s.payload ?? {}, null, 2),
    directory: s.directory ?? '',
    event: s.event ?? '',
    apply_to_shadow: s.apply_to_shadow ?? true,
  }))
}

export function MockStudioView(props: {
  rules: MockRule[]
  sseRules: SseMockRule[]
  openapi: OpenApiSummary | null
  activeDirectories: string[]
  onRefreshRules: () => void
  onRefreshSseRules: () => void
  onRefreshOpenApi: () => void
  onRefreshTapes: () => void
  onNotify: (msg: string, isErr?: boolean) => void
}) {
  const [subTab, setSubTab] = useState<'rules' | 'sse_rules' | 'openapi' | 'llm'>('rules')

  // ---- HTTP Rule Create / Edit state ----
  const [editingRuleId, setEditingRuleId] = useState<string | null>(null)
  const [ruleEnabled, setRuleEnabled] = useState(true)
  const [priority, setPriority] = useState(100)
  const [name, setName] = useState('Mock session status')
  const [method, setMethod] = useState('GET')
  const [pathPattern, setPathPattern] = useState('/session/status')
  const [queryPattern, setQueryPattern] = useState('')
  const [bodyContains, setBodyContains] = useState('')
  const [dirFilter, setDirFilter] = useState('')
  const [actionType, setActionType] = useState<MockActionType>('direct')
  const [bpPhase, setBpPhase] = useState<BreakpointPhase>('request')
  const [oneShot, setOneShot] = useState(false)
  const [status, setStatus] = useState(200)
  const [delayMs, setDelayMs] = useState(0)
  const [headersText, setHeadersText] = useState('{}')
  const [payloadText, setPayloadText] = useState('{\n  "ses_mock_001": { "type": "busy" }\n}')
  const [httpSseSteps, setHttpSseSteps] = useState<StepDraft[]>([])
  const [httpMutatePrompt, setHttpMutatePrompt] = useState('Change values to test error/edge-case UI rendering')
  const [httpMutating, setHttpMutating] = useState(false)

  // ---- SSE Rule Create / Edit state ----
  const [editingSseRuleId, setEditingSseRuleId] = useState<string | null>(null)
  const [sseEnabled, setSseEnabled] = useState(true)
  const [ssePriority, setSsePriority] = useState(100)
  const [sseName, setSseName] = useState('Intercept permission prompt')
  const [sseEventPattern, setSseEventPattern] = useState('permission.asked')
  const [sseSessionFilter, setSseSessionFilter] = useState('')
  const [sseDirFilter, setSseDirFilter] = useState('')
  const [ssePayloadContains, setSsePayloadContains] = useState('')
  const [sseAction, setSseAction] = useState<SseMockActionType>('patch')
  const [sseDelayMs, setSseDelayMs] = useState(0)
  const [sseOneShot, setSseOneShot] = useState(false)
  const [ssePatchText, setSsePatchText] = useState(
    '{\n  "properties": {\n    "patterns": ["mocked-command --safe"]\n  }\n}',
  )
  const [sseReplaceSteps, setSseReplaceSteps] = useState<StepDraft[]>([])

  // ---- OpenAPI Explorer (editable!) ----
  const [oaFilter, setOaFilter] = useState('')
  const [oaPreview, setOaPreview] = useState<{
    title: string
    method?: string
    path?: string
    eventType?: string
  } | null>(null)
  const [oaEditText, setOaEditText] = useState('')
  const [oaStatus, setOaStatus] = useState(200)
  const [oaPathEdit, setOaPathEdit] = useState('')
  const [syncingOa, setSyncingOa] = useState(false)

  // ---- LLM Studio (editable!) ----
  const [llmMode, setLlmMode] = useState<'scenario' | 'http'>('scenario')
  const [llmPrompt, setLlmPrompt] = useState(
    'Generate a realistic OpenCode session where the assistant reasons for 2 steps, runs a bash test command that fails once, asks for permission to edit src/main.rs, and streams a concise summary with a file diff before going idle.',
  )
  const [llmHttpMethod, setLlmHttpMethod] = useState('GET')
  const [llmHttpPath, setLlmHttpPath] = useState('/session/:sessionID/message')
  const [llmHttpStatus, setLlmHttpStatus] = useState(200)
  const [llmTapeName, setLlmTapeName] = useState('AI Scenario Replay')
  const [llmBusy, setLlmBusy] = useState(false)
  const [llmEditBody, setLlmEditBody] = useState<string | null>(null)
  const [llmMetaNote, setLlmMetaNote] = useState<string | null>(null)

  const resetHttpForm = () => {
    setEditingRuleId(null)
    setRuleEnabled(true)
    setPriority(100)
    setName('Mock session status')
    setMethod('GET')
    setPathPattern('/session/status')
    setQueryPattern('')
    setBodyContains('')
    setDirFilter('')
    setActionType('direct')
    setBpPhase('request')
    setOneShot(false)
    setStatus(200)
    setDelayMs(0)
    setHeadersText('{}')
    setPayloadText('{\n  "ses_mock_001": { "type": "busy" }\n}')
    setHttpSseSteps([])
  }

  const loadHttpRuleIntoForm = (r: MockRule, asCopy = false) => {
    setEditingRuleId(asCopy ? null : r.id)
    setRuleEnabled(r.enabled)
    setPriority(r.priority)
    setName(asCopy ? `${r.name} (copy)` : r.name)
    setMethod(r.method)
    setPathPattern(r.path_pattern)
    setQueryPattern(r.query_pattern ?? '')
    setBodyContains(r.body_contains ?? '')
    setDirFilter(r.directory_filter ?? '')
    setActionType(r.action_type)
    setBpPhase(r.breakpoint_phase ?? 'request')
    setOneShot(r.one_shot ?? false)
    setStatus(r.status)
    setDelayMs(r.delay_ms)
    setHeadersText(JSON.stringify(r.headers ?? {}, null, 2))
    const rawVal = r.action_type === 'patch' ? r.patch : r.body
    setPayloadText(
      rawVal === null || rawVal === undefined
        ? '{\n  \n}'
        : typeof rawVal === 'string'
          ? rawVal
          : JSON.stringify(rawVal, null, 2),
    )
    setHttpSseSteps(toStepDrafts(r.sse_sequence))
    setSubTab('rules')
  }

  const handleSaveHttpRule = async () => {
    if (!pathPattern.trim()) {
      props.onNotify('Path pattern is required', true)
      return
    }
    let headersObj: Record<string, string> = {}
    if (headersText.trim()) {
      try {
        headersObj = JSON.parse(headersText) as Record<string, string>
      } catch {
        props.onNotify('Headers must be a valid JSON object', true)
        return
      }
    }
    let parsed: unknown = undefined
    if (actionType !== 'breakpoint' && payloadText.trim()) {
      try {
        parsed = JSON.parse(payloadText)
      } catch {
        if (actionType === 'patch') {
          props.onNotify('JSON Merge Patch must be valid JSON', true)
          return
        }
        parsed = payloadText
      }
    }

    const sse_sequence: MockSseStep[] = []
    for (let i = 0; i < httpSseSteps.length; i++) {
      const s = httpSseSteps[i]
      try {
        sse_sequence.push({
          delay_ms: Math.max(0, Number(s.delay_ms) || 0),
          payload: JSON.parse(s.payloadText),
          directory: s.directory.trim() || null,
          event: s.event.trim() || null,
          apply_to_shadow: s.apply_to_shadow,
        })
      } catch {
        props.onNotify(`SSE sequence step #${i + 1} is not valid JSON`, true)
        return
      }
    }

    const input = {
      name: name.trim() || `${method} ${pathPattern}`,
      enabled: ruleEnabled,
      priority,
      method,
      path_pattern: pathPattern.trim(),
      query_pattern: queryPattern.trim() || null,
      body_contains: bodyContains.trim() || null,
      directory_filter: dirFilter.trim() || null,
      action_type: actionType,
      breakpoint_phase: bpPhase,
      one_shot: oneShot,
      status,
      delay_ms: delayMs,
      headers: headersObj,
      body: actionType === 'direct' || actionType === 'sse_stream' ? parsed : undefined,
      patch: actionType === 'patch' ? parsed : undefined,
      sse_sequence,
    }

    try {
      if (editingRuleId) {
        await api.updateRule(editingRuleId, input)
        props.onNotify(`Updated HTTP rule "${input.name}"`)
      } else {
        await api.createRule(input)
        props.onNotify(`Created ${actionType} rule for ${method} ${pathPattern}`)
      }
      props.onRefreshRules()
      if (editingRuleId) setEditingRuleId(null)
    } catch (e) {
      props.onNotify(String(e), true)
    }
  }

  const toggleRule = async (r: MockRule) => {
    try {
      await api.updateRule(r.id, {
        name: r.name,
        enabled: !r.enabled,
        priority: r.priority,
        method: r.method,
        path_pattern: r.path_pattern,
        query_pattern: r.query_pattern,
        body_contains: r.body_contains,
        directory_filter: r.directory_filter,
        action_type: r.action_type,
        breakpoint_phase: r.breakpoint_phase,
        one_shot: r.one_shot,
        status: r.status,
        delay_ms: r.delay_ms,
        headers: r.headers,
        body: r.body,
        patch: r.patch,
        sse_sequence: r.sse_sequence,
      })
      props.onRefreshRules()
    } catch (e) {
      props.onNotify(String(e), true)
    }
  }

  const deleteRule = async (id: string) => {
    try {
      await api.deleteRule(id)
      if (editingRuleId === id) resetHttpForm()
      props.onRefreshRules()
      props.onNotify('Deleted HTTP mock rule')
    } catch (e) {
      props.onNotify(String(e), true)
    }
  }

  // ---- SSE Rules handlers ----
  const resetSseForm = () => {
    setEditingSseRuleId(null)
    setSseEnabled(true)
    setSsePriority(100)
    setSseName('Intercept permission prompt')
    setSseEventPattern('permission.asked')
    setSseSessionFilter('')
    setSseDirFilter('')
    setSsePayloadContains('')
    setSseAction('patch')
    setSseDelayMs(0)
    setSseOneShot(false)
    setSsePatchText('{\n  "properties": {\n    "patterns": ["mocked-command --safe"]\n  }\n}')
    setSseReplaceSteps([])
  }

  const loadSseRuleIntoForm = (r: SseMockRule, asCopy = false) => {
    setEditingSseRuleId(asCopy ? null : r.id)
    setSseEnabled(r.enabled)
    setSsePriority(r.priority)
    setSseName(asCopy ? `${r.name} (copy)` : r.name)
    setSseEventPattern(r.event_type_pattern)
    setSseSessionFilter(r.session_filter ?? '')
    setSseDirFilter(r.directory_filter ?? '')
    setSsePayloadContains(r.payload_contains ?? '')
    setSseAction(r.action_type)
    setSseDelayMs(r.delay_ms)
    setSseOneShot(r.one_shot)
    setSsePatchText(r.patch ? JSON.stringify(r.patch, null, 2) : '{\n  \n}')
    setSseReplaceSteps(toStepDrafts(r.replace_events))
    setSubTab('sse_rules')
  }

  const handleSaveSseRule = async () => {
    if (!sseEventPattern.trim()) {
      props.onNotify('Event type pattern is required', true)
      return
    }
    let patchVal: unknown = undefined
    if (sseAction === 'patch' && ssePatchText.trim()) {
      try {
        patchVal = JSON.parse(ssePatchText)
      } catch {
        props.onNotify('SSE JSON Merge Patch must be valid JSON', true)
        return
      }
    }

    const replace_events: MockSseStep[] = []
    if (sseAction === 'replace') {
      if (sseReplaceSteps.length === 0) {
        try {
          replace_events.push({
            delay_ms: 0,
            payload: JSON.parse(ssePatchText),
            apply_to_shadow: true,
          })
        } catch {
          props.onNotify('Replacement SSE payload must be valid JSON', true)
          return
        }
      } else {
        for (let i = 0; i < sseReplaceSteps.length; i++) {
          const step = sseReplaceSteps[i]
          try {
            replace_events.push({
              delay_ms: Math.max(0, Number(step.delay_ms) || 0),
              payload: JSON.parse(step.payloadText),
              directory: step.directory.trim() || null,
              apply_to_shadow: step.apply_to_shadow,
            })
          } catch {
            props.onNotify(`Replacement frame #${i + 1} is not valid JSON`, true)
            return
          }
        }
      }
    }

    const input = {
      name: sseName.trim() || `${sseAction} ${sseEventPattern.trim()}`,
      enabled: sseEnabled,
      priority: ssePriority,
      event_type_pattern: sseEventPattern.trim(),
      session_filter: sseSessionFilter.trim() || null,
      directory_filter: sseDirFilter.trim() || null,
      payload_contains: ssePayloadContains.trim() || null,
      action_type: sseAction,
      delay_ms: sseDelayMs,
      patch: sseAction === 'patch' ? patchVal : undefined,
      replace_events,
      one_shot: sseOneShot,
    }

    try {
      if (editingSseRuleId) {
        await api.updateSseRule(editingSseRuleId, input)
        props.onNotify(`Updated SSE rule "${input.name}"`)
      } else {
        await api.createSseRule(input)
        props.onNotify(`Created SSE ${sseAction} rule for ${sseEventPattern.trim()}`)
      }
      props.onRefreshSseRules()
      if (editingSseRuleId) setEditingSseRuleId(null)
    } catch (e) {
      props.onNotify(String(e), true)
    }
  }

  const toggleSseRule = async (r: SseMockRule) => {
    try {
      await api.updateSseRule(r.id, {
        name: r.name,
        enabled: !r.enabled,
        priority: r.priority,
        event_type_pattern: r.event_type_pattern,
        session_filter: r.session_filter,
        directory_filter: r.directory_filter,
        payload_contains: r.payload_contains,
        action_type: r.action_type,
        delay_ms: r.delay_ms,
        patch: r.patch,
        replace_events: r.replace_events,
        one_shot: r.one_shot,
      })
      props.onRefreshSseRules()
    } catch (e) {
      props.onNotify(String(e), true)
    }
  }

  const deleteSseRule = async (id: string) => {
    try {
      await api.deleteSseRule(id)
      if (editingSseRuleId === id) resetSseForm()
      props.onRefreshSseRules()
      props.onNotify('Deleted SSE mock rule')
    } catch (e) {
      props.onNotify(String(e), true)
    }
  }

  const syncOpenApi = async () => {
    setSyncingOa(true)
    try {
      await api.syncOpenApi()
      props.onRefreshOpenApi()
      props.onNotify('Synced OpenAPI /doc from upstream OpenCode server')
    } catch (e) {
      props.onNotify(String(e), true)
    } finally {
      setSyncingOa(false)
    }
  }

  const fakeOp = async (m: string, p: string) => {
    try {
      const data = await api.fakeHttp(m, p, props.activeDirectories[0])
      setOaPreview({ title: `${m} ${p}`, method: m, path: p })
      setOaPathEdit(p)
      setOaStatus(200)
      setOaEditText(JSON.stringify(data, null, 2))
    } catch (e) {
      props.onNotify(String(e), true)
    }
  }

  const fakeEv = async (evType: string) => {
    try {
      const data = await api.fakeSse(evType, props.activeDirectories[0])
      setOaPreview({ title: `SSE ${evType}`, eventType: evType })
      setOaEditText(JSON.stringify(data, null, 2))
    } catch (e) {
      props.onNotify(String(e), true)
    }
  }

  const savePreviewAsMock = async () => {
    if (!oaPreview?.method) return
    let bodyVal: unknown = oaEditText
    try {
      bodyVal = JSON.parse(oaEditText)
    } catch {}
    try {
      await api.createRule({
        name: `Mock ${oaPreview.method} ${oaPathEdit || oaPreview.path}`,
        method: oaPreview.method,
        path_pattern: (oaPathEdit || oaPreview.path || '/').trim(),
        action_type: 'direct',
        status: oaStatus,
        body: bodyVal,
      })
      props.onRefreshRules()
      props.onNotify(`Saved edited mock rule for ${oaPreview.method} ${oaPathEdit || oaPreview.path}`)
    } catch (e) {
      props.onNotify(String(e), true)
    }
  }

  const savePreviewAsSseRule = async () => {
    if (!oaPreview?.eventType) return
    let parsed: unknown
    try {
      parsed = JSON.parse(oaEditText)
    } catch {
      props.onNotify('SSE JSON payload is not valid JSON', true)
      return
    }
    try {
      await api.createSseRule({
        name: `Replace ${oaPreview.eventType}`,
        event_type_pattern: oaPreview.eventType,
        action_type: 'replace',
        replace_events: [{ delay_ms: 0, payload: parsed, apply_to_shadow: true }],
      })
      props.onRefreshSseRules()
      props.onNotify(`Saved SSE replacement rule for ${oaPreview.eventType}`)
    } catch (e) {
      props.onNotify(String(e), true)
    }
  }

  const runLlm = async () => {
    setLlmBusy(true)
    setLlmEditBody(null)
    setLlmMetaNote(null)
    try {
      if (llmMode === 'scenario') {
        const res = await api.llmGenerateScenario({
          prompt: llmPrompt,
          directory: props.activeDirectories[0],
          tape_name: llmTapeName,
          save_tape: true,
        })
        props.onRefreshTapes()
        setLlmEditBody(JSON.stringify(res, null, 2))
        setLlmMetaNote(
          `Saved tape "${res.tape.name}" (${res.validated ? 'schema-verified' : 'with warnings'}, ${res.attempts} attempt(s)). Edit individual SSE frames anytime under Replay & Tapes.`,
        )
        props.onNotify(`Generated & saved tape "${res.tape.name}"`)
      } else {
        const res = await api.llmGenerateHttp({
          method: llmHttpMethod,
          path: llmHttpPath,
          prompt: llmPrompt,
          directory: props.activeDirectories[0],
        })
        setLlmEditBody(JSON.stringify(res.data, null, 2))
        setLlmMetaNote(
          `Schema-${res.validated ? 'verified' : 'checked'} in ${res.attempts} attempt(s). You can manually edit the JSON below before saving as a rule.`,
        )
        props.onNotify('Generated HTTP mock — edit below and click Save as HTTP Mock Rule')
      }
    } catch (e) {
      props.onNotify(String(e), true)
    } finally {
      setLlmBusy(false)
    }
  }

  const addHttpSseStep = async (evType = 'session.status') => {
    let sample: unknown = {
      type: evType,
      properties: { sessionID: '{{session_id}}', status: { type: 'busy' } },
    }
    try {
      const faked = await api.fakeSse(evType, '{{directory}}')
      sample = JSON.parse(JSON.stringify(faked).replace(/"ses_[^"]*"/g, '"{{session_id}}"'))
    } catch {}
    setHttpSseSteps((prev) => [
      ...prev,
      {
        delay_ms: prev.length === 0 ? 50 : 150,
        payloadText: JSON.stringify(sample, null, 2),
        directory: '',
        event: '',
        apply_to_shadow: true,
      },
    ])
  }

  const filteredOps = (props.openapi?.operations ?? []).filter((op) => {
    const q = oaFilter.trim().toLowerCase()
    if (!q) return true
    return (
      op.path.toLowerCase().includes(q) ||
      op.method.toLowerCase().includes(q) ||
      (op.operation_id ?? '').toLowerCase().includes(q)
    )
  })

  const filteredEvents = (props.openapi?.events ?? []).filter((ev) =>
    ev.event_type.toLowerCase().includes(oaFilter.trim().toLowerCase()),
  )

  return (
    <>
      <div className="tab-strip">
        <button className={`tab-btn ${subTab === 'rules' ? 'active' : ''}`} onClick={() => setSubTab('rules')}>
          HTTP Rules ({props.rules.length})
        </button>
        <button className={`tab-btn ${subTab === 'sse_rules' ? 'active' : ''}`} onClick={() => setSubTab('sse_rules')}>
          SSE Stream Rules ({props.sseRules.length})
        </button>
        <button className={`tab-btn ${subTab === 'openapi' ? 'active' : ''}`} onClick={() => setSubTab('openapi')}>
          OpenAPI /doc Faker ({props.openapi?.operations.length ?? 0} APIs / {props.openapi?.events.length ?? 0} Events)
        </button>
        <button className={`tab-btn ${subTab === 'llm' ? 'active' : ''}`} onClick={() => setSubTab('llm')}>
          AI Scenario &amp; Mock Generator
        </button>
      </div>

      {subTab === 'rules' && (
        <div style={{ flex: 1, overflow: 'auto', padding: 14, display: 'flex', flexDirection: 'column', gap: 14 }}>
          <div className="panel">
            <div className="panel-title">
              <span>
                {editingRuleId
                  ? `Editing HTTP Rule (${editingRuleId})`
                  : 'Create HTTP Mock / Patch / SSE Stream / Breakpoint Rule'}
              </span>
              {editingRuleId && (
                <button className="btn btn-sm" onClick={resetHttpForm}>
                  <X size={12} /> Cancel Edit (New Rule)
                </button>
              )}
            </div>

            <div className="form-grid" style={{ marginBottom: 10 }}>
              <div className="field">
                <label>Rule Name</label>
                <input className="input" value={name} onChange={(e) => setName(e.target.value)} />
              </div>
              <div className="field">
                <label>Method</label>
                <select className="select" value={method} onChange={(e) => setMethod(e.target.value)}>
                  <option value="ANY">ANY</option>
                  <option value="GET">GET</option>
                  <option value="POST">POST</option>
                  <option value="PUT">PUT</option>
                  <option value="PATCH">PATCH</option>
                  <option value="DELETE">DELETE</option>
                </select>
              </div>
              <div className="field">
                <label>Path Pattern (:param, *, or regex:...)</label>
                <input
                  className="input mono"
                  value={pathPattern}
                  onChange={(e) => setPathPattern(e.target.value)}
                  placeholder="/session/:sessionID/message or regex:^/session/.*"
                />
              </div>
              <div className="field">
                <label>Action</label>
                <select
                  className="select"
                  value={actionType}
                  onChange={(e) => setActionType(e.target.value as MockActionType)}
                >
                  <option value="direct">Direct Mock Response</option>
                  <option value="patch">JSON Merge Patch Upstream</option>
                  <option value="sse_stream">Direct SSE Stream (text/event-stream)</option>
                  <option value="breakpoint">Dynamic Breakpoint</option>
                </select>
              </div>
            </div>

            <div className="form-grid" style={{ marginBottom: 10 }}>
              <div className="field">
                <label>Query Pattern (optional, * or regex:)</label>
                <input
                  className="input mono"
                  value={queryPattern}
                  onChange={(e) => setQueryPattern(e.target.value)}
                  placeholder="e.g. directory=*demo*"
                />
              </div>
              <div className="field">
                <label>Req Body Contains (optional, * or regex:)</label>
                <input
                  className="input mono"
                  value={bodyContains}
                  onChange={(e) => setBodyContains(e.target.value)}
                  placeholder="Match keyword or regex in body"
                />
              </div>
              <div className="field">
                <label>Directory Filter (optional)</label>
                <input
                  className="input mono"
                  value={dirFilter}
                  onChange={(e) => setDirFilter(e.target.value)}
                  placeholder="Any directory"
                />
              </div>
              <div className="field">
                <label>Priority (lower runs first)</label>
                <input
                  className="input mono"
                  type="number"
                  value={priority}
                  onChange={(e) => setPriority(Number(e.target.value))}
                />
              </div>
            </div>

            <div className="form-grid" style={{ marginBottom: 10 }}>
              {actionType === 'breakpoint' ? (
                <div className="field">
                  <label>Breakpoint Phase</label>
                  <select
                    className="select"
                    value={bpPhase}
                    onChange={(e) => setBpPhase(e.target.value as BreakpointPhase)}
                  >
                    <option value="request">Request (Before Upstream)</option>
                    <option value="response">Response (Edit Real Upstream Response)</option>
                    <option value="both">Both Request &amp; Response</option>
                  </select>
                </div>
              ) : (
                <div className="field">
                  <label>HTTP Status</label>
                  <input
                    className="input mono"
                    type="number"
                    value={status}
                    onChange={(e) => setStatus(Number(e.target.value))}
                  />
                </div>
              )}
              <div className="field">
                <label>Artificial Delay (ms)</label>
                <input
                  className="input mono"
                  type="number"
                  value={delayMs}
                  onChange={(e) => setDelayMs(Number(e.target.value))}
                />
              </div>
              <div className="field" style={{ justifyContent: 'flex-end' }}>
                <label style={{ display: 'inline-flex', alignItems: 'center', gap: 6, cursor: 'pointer' }}>
                  <input type="checkbox" checked={oneShot} onChange={(e) => setOneShot(e.target.checked)} />
                  One-shot (auto-disable after 1 hit)
                </label>
              </div>
            </div>

            {actionType !== 'breakpoint' && (
              <>
                <div className="field" style={{ marginBottom: 10 }}>
                  <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
                    <label>
                      {actionType === 'patch'
                        ? 'JSON Merge Patch Object (null removes key)'
                        : actionType === 'sse_stream'
                          ? 'Fallback Single Event Payload (or configure multi-step stream below)'
                          : 'Mock Response Body (JSON or raw text; supports {{session_id}} & {{directory}})'}
                    </label>
                    <div className="toolbar-group" style={{ gap: 6 }}>
                      <button
                        className="btn btn-sm"
                        onClick={() => {
                          try {
                            setPayloadText(JSON.stringify(JSON.parse(payloadText), null, 2))
                          } catch {
                            props.onNotify('Not valid JSON', true)
                          }
                        }}
                      >
                        Format JSON
                      </button>
                      <button
                        className="btn btn-sm"
                        onClick={async () => {
                          try {
                            const fake = await api.fakeHttp(
                              method === 'ANY' ? 'GET' : method,
                              pathPattern,
                              props.activeDirectories[0],
                            )
                            setPayloadText(JSON.stringify(fake, null, 2))
                            props.onNotify('Filled body from OpenAPI Schema Faker')
                          } catch (e) {
                            props.onNotify(String(e), true)
                          }
                        }}
                      >
                        <Wand2 size={11} /> Auto-fill OpenAPI
                      </button>
                    </div>
                  </div>
                  <textarea
                    className="textarea"
                    rows={6}
                    value={payloadText}
                    onChange={(e) => setPayloadText(e.target.value)}
                  />
                </div>

                <div style={{ display: 'flex', gap: 8, marginBottom: 10, alignItems: 'center' }}>
                  <input
                    className="input"
                    style={{ flex: 1 }}
                    placeholder="Optional AI Mutate instruction on the JSON body above..."
                    value={httpMutatePrompt}
                    onChange={(e) => setHttpMutatePrompt(e.target.value)}
                  />
                  <button
                    className="btn btn-sm"
                    disabled={httpMutating}
                    onClick={async () => {
                      let parsed: unknown
                      try {
                        parsed = JSON.parse(payloadText)
                      } catch {
                        props.onNotify('Body must be valid JSON to run AI Mutate', true)
                        return
                      }
                      setHttpMutating(true)
                      try {
                        const res = await api.llmMutate({ payload: parsed, instruction: httpMutatePrompt })
                        setPayloadText(JSON.stringify(res.data, null, 2))
                        props.onNotify('AI mutation applied to editor — tweak manually as needed')
                      } catch (e) {
                        props.onNotify(String(e), true)
                      } finally {
                        setHttpMutating(false)
                      }
                    }}
                  >
                    <Sparkles size={12} /> {httpMutating ? 'Mutating...' : 'AI Mutate Body'}
                  </button>
                </div>

                <div className="field" style={{ marginBottom: 10 }}>
                  <label>Custom Response Headers (JSON object)</label>
                  <textarea
                    className="textarea"
                    rows={2}
                    value={headersText}
                    onChange={(e) => setHeadersText(e.target.value)}
                  />
                </div>

                <div className="panel" style={{ padding: 10, marginBottom: 10, background: 'var(--bg-inset)' }}>
                  <div className="panel-title" style={{ marginBottom: 6, fontSize: 12 }}>
                    <span>
                      {actionType === 'sse_stream'
                        ? `Streamed HTTP SSE Frames (${httpSseSteps.length})`
                        : `Linked SSE Hub Sequence Triggered by HTTP Match (${httpSseSteps.length})`}
                    </span>
                    <button className="btn btn-sm" onClick={() => addHttpSseStep('session.status')}>
                      <Plus size={11} /> Add SSE Frame
                    </button>
                  </div>
                  {httpSseSteps.length === 0 ? (
                    <div style={{ fontSize: 11.5, color: 'var(--text-muted)' }}>
                      Optional: attach timed SSE frames (e.g. <code>session.status</code> → <code>message.part.delta</code> → <code>session.idle</code>) that emit automatically when a client calls this mocked HTTP endpoint.
                    </div>
                  ) : (
                    <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
                      {httpSseSteps.map((step, idx) => (
                        <div key={idx} className="panel" style={{ padding: 8 }}>
                          <div className="toolbar-group" style={{ justifyContent: 'space-between', marginBottom: 6 }}>
                            <div className="toolbar-group">
                              <span className="badge badge-info">Step #{idx + 1}</span>
                              <label style={{ fontSize: 11, color: 'var(--text-secondary)' }}>+Delay(ms):</label>
                              <input
                                className="input mono"
                                style={{ width: 76 }}
                                type="number"
                                value={step.delay_ms}
                                onChange={(e) => {
                                  const v = Number(e.target.value)
                                  setHttpSseSteps((prev) =>
                                    prev.map((s, i) => (i === idx ? { ...s, delay_ms: v } : s)),
                                  )
                                }}
                              />
                              <select
                                className="select"
                                defaultValue=""
                                onChange={async (e) => {
                                  const evType = e.target.value
                                  if (!evType) return
                                  try {
                                    const faked = await api.fakeSse(evType, '{{directory}}')
                                    const text = JSON.stringify(faked, null, 2).replace(
                                      /"ses_[^"]*"/g,
                                      '"{{session_id}}"',
                                    )
                                    setHttpSseSteps((prev) =>
                                      prev.map((s, i) => (i === idx ? { ...s, payloadText: text } : s)),
                                    )
                                  } catch {}
                                }}
                              >
                                <option value="">Fill from OpenAPI event...</option>
                                {(props.openapi?.events ?? []).map((ev) => (
                                  <option key={ev.event_type} value={ev.event_type}>
                                    {ev.event_type}
                                  </option>
                                ))}
                              </select>
                              {actionType === 'sse_stream' ? (
                                <input
                                  className="input mono"
                                  style={{ width: 130 }}
                                  placeholder="event: (optional)"
                                  value={step.event}
                                  onChange={(e) => {
                                    const v = e.target.value
                                    setHttpSseSteps((prev) =>
                                      prev.map((s, i) => (i === idx ? { ...s, event: v } : s)),
                                    )
                                  }}
                                />
                              ) : (
                                <label style={{ display: 'inline-flex', alignItems: 'center', gap: 4, fontSize: 11 }}>
                                  <input
                                    type="checkbox"
                                    checked={step.apply_to_shadow}
                                    onChange={(e) => {
                                      const v = e.target.checked
                                      setHttpSseSteps((prev) =>
                                        prev.map((s, i) => (i === idx ? { ...s, apply_to_shadow: v } : s)),
                                      )
                                    }}
                                  />
                                  ShadowStore
                                </label>
                              )}
                            </div>
                            <button
                              className="btn btn-sm btn-danger"
                              onClick={() => setHttpSseSteps((prev) => prev.filter((_, i) => i !== idx))}
                            >
                              <Trash2 size={11} />
                            </button>
                          </div>
                          <textarea
                            className="textarea"
                            rows={4}
                            value={step.payloadText}
                            onChange={(e) => {
                              const val = e.target.value
                              setHttpSseSteps((prev) =>
                                prev.map((s, i) => (i === idx ? { ...s, payloadText: val } : s)),
                              )
                            }}
                          />
                        </div>
                      ))}
                    </div>
                  )}
                </div>
              </>
            )}

            <div className="toolbar-group">
              <button className="btn btn-primary" onClick={handleSaveHttpRule}>
                {editingRuleId ? <Save size={13} /> : <Plus size={13} />}
                {editingRuleId ? 'Save Rule Changes' : 'Add HTTP Rule'}
              </button>
            </div>
          </div>

          <div className="panel" style={{ padding: 0, overflow: 'hidden' }}>
            <table className="data-table">
              <thead>
                <tr>
                  <th>State</th>
                  <th>Prio</th>
                  <th>Action</th>
                  <th>Method</th>
                  <th>Pattern &amp; Dynamic Matchers</th>
                  <th>Status / Delay</th>
                  <th>Hits</th>
                  <th>Name</th>
                  <th></th>
                </tr>
              </thead>
              <tbody>
                {props.rules.length === 0 ? (
                  <tr>
                    <td colSpan={9} className="empty-state">
                      No HTTP mock rules configured.
                    </td>
                  </tr>
                ) : (
                  props.rules.map((r) => (
                    <tr
                      key={r.id}
                      className={editingRuleId === r.id ? 'selected' : ''}
                      onClick={() => loadHttpRuleIntoForm(r)}
                    >
                      <td onClick={(e) => e.stopPropagation()}>
                        <button
                          className={`btn btn-sm ${r.enabled ? 'btn-primary' : ''}`}
                          onClick={() => toggleRule(r)}
                        >
                          {r.enabled ? 'ON' : 'OFF'}
                        </button>
                      </td>
                      <td className="mono" style={{ color: 'var(--text-muted)' }}>
                        {r.priority}
                      </td>
                      <td>
                        <span
                          className={`badge ${
                            r.action_type === 'breakpoint'
                              ? 'badge-err'
                              : r.action_type === 'patch'
                                ? 'badge-info'
                                : r.action_type === 'sse_stream'
                                  ? 'badge-ok'
                                  : 'badge-warn'
                          }`}
                        >
                          {r.action_type === 'breakpoint' ? `bp:${r.breakpoint_phase}` : r.action_type}
                        </span>
                        {r.sse_sequence?.length > 0 && (
                          <span className="badge badge-info" style={{ marginLeft: 4 }}>
                            +{r.sse_sequence.length} SSE
                          </span>
                        )}
                        {r.one_shot && (
                          <span className="badge badge-neutral" style={{ marginLeft: 4 }}>
                            1-shot
                          </span>
                        )}
                      </td>
                      <td className={`mono method-${r.method}`} style={{ fontWeight: 600 }}>
                        {r.method}
                      </td>
                      <td className="mono">
                        {r.path_pattern}
                        {r.query_pattern && (
                          <span style={{ color: 'var(--text-muted)', marginLeft: 6 }}>?{r.query_pattern}</span>
                        )}
                        {r.body_contains && (
                          <span className="badge badge-neutral" style={{ marginLeft: 6 }}>
                            body~{r.body_contains}
                          </span>
                        )}
                      </td>
                      <td className="mono">
                        {r.action_type === 'breakpoint' ? 'suspend' : r.status}{' '}
                        {r.delay_ms > 0 ? `+${r.delay_ms}ms` : ''}
                      </td>
                      <td className="mono" style={{ color: 'var(--text-secondary)' }}>
                        {r.hit_count ?? 0}
                      </td>
                      <td>{r.name}</td>
                      <td style={{ textAlign: 'right' }} onClick={(e) => e.stopPropagation()}>
                        <div className="toolbar-group" style={{ justifyContent: 'flex-end', gap: 4 }}>
                          <button className="btn btn-sm" onClick={() => loadHttpRuleIntoForm(r)} title="Edit rule">
                            <Edit3 size={12} />
                          </button>
                          <button
                            className="btn btn-sm"
                            onClick={() => loadHttpRuleIntoForm(r, true)}
                            title="Duplicate rule"
                          >
                            <Copy size={12} />
                          </button>
                          <button className="btn btn-sm btn-danger" onClick={() => deleteRule(r.id)} title="Delete">
                            <Trash2 size={12} />
                          </button>
                        </div>
                      </td>
                    </tr>
                  ))
                )}
              </tbody>
            </table>
          </div>
        </div>
      )}

      {subTab === 'sse_rules' && (
        <div style={{ flex: 1, overflow: 'auto', padding: 14, display: 'flex', flexDirection: 'column', gap: 14 }}>
          <div className="panel">
            <div className="panel-title">
              <span>
                {editingSseRuleId
                  ? `Editing SSE Stream Rule (${editingSseRuleId})`
                  : 'Create Live SSE Stream Mock / Drop / Patch / Breakpoint Rule'}
              </span>
              {editingSseRuleId && (
                <button className="btn btn-sm" onClick={resetSseForm}>
                  <X size={12} /> Cancel Edit (New Rule)
                </button>
              )}
            </div>

            <div className="form-grid" style={{ marginBottom: 10 }}>
              <div className="field">
                <label>Rule Name</label>
                <input className="input" value={sseName} onChange={(e) => setSseName(e.target.value)} />
              </div>
              <div className="field">
                <label>Event Type Pattern (* or regex: supported)</label>
                <input
                  className="input mono"
                  list="sse-event-types-list"
                  value={sseEventPattern}
                  onChange={(e) => setSseEventPattern(e.target.value)}
                  placeholder="message.part.* or permission.asked"
                />
                <datalist id="sse-event-types-list">
                  <option value="*" />
                  <option value="message.part.*" />
                  <option value="session.*" />
                  {(props.openapi?.events ?? []).map((ev) => (
                    <option key={ev.event_type} value={ev.event_type} />
                  ))}
                </datalist>
              </div>
              <div className="field">
                <label>Action</label>
                <select
                  className="select"
                  value={sseAction}
                  onChange={(e) => setSseAction(e.target.value as SseMockActionType)}
                >
                  <option value="patch">JSON Merge Patch Payload</option>
                  <option value="replace">Replace with Custom Event(s)</option>
                  <option value="drop">Drop / Suppress Event</option>
                  <option value="delay">Delay Delivery Only</option>
                  <option value="breakpoint">Live SSE Breakpoint (Suspend &amp; Edit)</option>
                </select>
              </div>
              <div className="field">
                <label>Priority</label>
                <input
                  className="input mono"
                  type="number"
                  value={ssePriority}
                  onChange={(e) => setSsePriority(Number(e.target.value))}
                />
              </div>
            </div>

            <div className="form-grid" style={{ marginBottom: 10 }}>
              <div className="field">
                <label>Session ID Filter (optional, * or regex:)</label>
                <input
                  className="input mono"
                  value={sseSessionFilter}
                  onChange={(e) => setSseSessionFilter(e.target.value)}
                  placeholder="Any session"
                />
              </div>
              <div className="field">
                <label>Payload Contains (optional, * or regex:)</label>
                <input
                  className="input mono"
                  value={ssePayloadContains}
                  onChange={(e) => setSsePayloadContains(e.target.value)}
                  placeholder='e.g. "tool":"bash"'
                />
              </div>
              <div className="field">
                <label>Directory Filter (optional)</label>
                <input
                  className="input mono"
                  value={sseDirFilter}
                  onChange={(e) => setSseDirFilter(e.target.value)}
                  placeholder="Any directory"
                />
              </div>
              <div className="field">
                <label>Delay (ms)</label>
                <input
                  className="input mono"
                  type="number"
                  value={sseDelayMs}
                  onChange={(e) => setSseDelayMs(Number(e.target.value))}
                />
              </div>
            </div>

            <label style={{ display: 'inline-flex', alignItems: 'center', gap: 6, fontSize: 12, marginBottom: 10 }}>
              <input type="checkbox" checked={sseOneShot} onChange={(e) => setSseOneShot(e.target.checked)} />
              One-shot rule (automatically disable after 1 matched frame — recommended when debugging <code>message.part.delta</code>)
            </label>

            {sseAction === 'patch' && (
              <div className="field" style={{ marginBottom: 10 }}>
                <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
                  <label>JSON Merge Patch applied onto SSE event payload</label>
                  <button
                    className="btn btn-sm"
                    onClick={() => {
                      try {
                        setSsePatchText(JSON.stringify(JSON.parse(ssePatchText), null, 2))
                      } catch {
                        props.onNotify('Not valid JSON', true)
                      }
                    }}
                  >
                    Format JSON
                  </button>
                </div>
                <textarea
                  className="textarea"
                  rows={6}
                  value={ssePatchText}
                  onChange={(e) => setSsePatchText(e.target.value)}
                />
              </div>
            )}

            {sseAction === 'replace' && (
              <div className="panel" style={{ padding: 10, marginBottom: 10, background: 'var(--bg-inset)' }}>
                <div className="panel-title" style={{ fontSize: 12, marginBottom: 8 }}>
                  <span>Replacement SSE Frame(s) ({sseReplaceSteps.length})</span>
                  <button
                    className="btn btn-sm"
                    onClick={async () => {
                      let sample: unknown = {
                        type: sseEventPattern.includes('*') ? 'session.error' : sseEventPattern,
                        properties: { sessionID: '{{session_id}}' },
                      }
                      try {
                        if (!sseEventPattern.includes('*') && !sseEventPattern.startsWith('regex:')) {
                          sample = await api.fakeSse(sseEventPattern, props.activeDirectories[0])
                        }
                      } catch {}
                      setSseReplaceSteps((prev) => [
                        ...prev,
                        {
                          delay_ms: 0,
                          payloadText: JSON.stringify(sample, null, 2),
                          directory: '',
                          event: '',
                          apply_to_shadow: true,
                        },
                      ])
                    }}
                  >
                    <Plus size={11} /> Add Replacement Frame
                  </button>
                </div>
                {sseReplaceSteps.length === 0 ? (
                  <div className="field">
                    <label>Single Replacement SSE Payload JSON</label>
                    <textarea
                      className="textarea"
                      rows={6}
                      value={ssePatchText}
                      onChange={(e) => setSsePatchText(e.target.value)}
                    />
                  </div>
                ) : (
                  <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
                    {sseReplaceSteps.map((step, idx) => (
                      <div key={idx} className="panel" style={{ padding: 8 }}>
                        <div className="toolbar-group" style={{ justifyContent: 'space-between', marginBottom: 6 }}>
                          <div className="toolbar-group">
                            <span className="badge badge-info">Frame #{idx + 1}</span>
                            <label style={{ fontSize: 11 }}>+Delay(ms):</label>
                            <input
                              className="input mono"
                              style={{ width: 76 }}
                              type="number"
                              value={step.delay_ms}
                              onChange={(e) => {
                                const v = Number(e.target.value)
                                setSseReplaceSteps((prev) =>
                                  prev.map((s, i) => (i === idx ? { ...s, delay_ms: v } : s)),
                                )
                              }}
                            />
                            <select
                              className="select"
                              defaultValue=""
                              onChange={async (e) => {
                                const evType = e.target.value
                                if (!evType) return
                                try {
                                  const faked = await api.fakeSse(evType, props.activeDirectories[0])
                                  setSseReplaceSteps((prev) =>
                                    prev.map((s, i) =>
                                      i === idx ? { ...s, payloadText: JSON.stringify(faked, null, 2) } : s,
                                    ),
                                  )
                                } catch {}
                              }}
                            >
                              <option value="">Template from OpenAPI...</option>
                              {(props.openapi?.events ?? []).map((ev) => (
                                <option key={ev.event_type} value={ev.event_type}>
                                  {ev.event_type}
                                </option>
                              ))}
                            </select>
                          </div>
                          <button
                            className="btn btn-sm btn-danger"
                            onClick={() => setSseReplaceSteps((prev) => prev.filter((_, i) => i !== idx))}
                          >
                            <Trash2 size={11} />
                          </button>
                        </div>
                        <textarea
                          className="textarea"
                          rows={4}
                          value={step.payloadText}
                          onChange={(e) => {
                            const val = e.target.value
                            setSseReplaceSteps((prev) =>
                              prev.map((s, i) => (i === idx ? { ...s, payloadText: val } : s)),
                            )
                          }}
                        />
                      </div>
                    ))}
                  </div>
                )}
              </div>
            )}

            <div className="toolbar-group">
              <button className="btn btn-primary" onClick={handleSaveSseRule}>
                {editingSseRuleId ? <Save size={13} /> : <Plus size={13} />}
                {editingSseRuleId ? 'Save SSE Rule Changes' : 'Add SSE Rule'}
              </button>
            </div>
          </div>

          <div className="panel" style={{ padding: 0, overflow: 'hidden' }}>
            <table className="data-table">
              <thead>
                <tr>
                  <th>State</th>
                  <th>Prio</th>
                  <th>Action</th>
                  <th>Event Pattern &amp; Dynamic Filters</th>
                  <th>Delay</th>
                  <th>Hits</th>
                  <th>Name</th>
                  <th></th>
                </tr>
              </thead>
              <tbody>
                {props.sseRules.length === 0 ? (
                  <tr>
                    <td colSpan={8} className="empty-state">
                      No SSE stream rules configured. Create one above to drop, patch, replace, delay, or breakpoint live OpenCode SSE frames.
                    </td>
                  </tr>
                ) : (
                  props.sseRules.map((r) => (
                    <tr
                      key={r.id}
                      className={editingSseRuleId === r.id ? 'selected' : ''}
                      onClick={() => loadSseRuleIntoForm(r)}
                    >
                      <td onClick={(e) => e.stopPropagation()}>
                        <button
                          className={`btn btn-sm ${r.enabled ? 'btn-primary' : ''}`}
                          onClick={() => toggleSseRule(r)}
                        >
                          {r.enabled ? 'ON' : 'OFF'}
                        </button>
                      </td>
                      <td className="mono" style={{ color: 'var(--text-muted)' }}>
                        {r.priority}
                      </td>
                      <td>
                        <span
                          className={`badge ${
                            r.action_type === 'drop' || r.action_type === 'breakpoint'
                              ? 'badge-err'
                              : r.action_type === 'patch'
                                ? 'badge-info'
                                : 'badge-warn'
                          }`}
                        >
                          {r.action_type}
                        </span>
                        {r.one_shot && (
                          <span className="badge badge-neutral" style={{ marginLeft: 4 }}>
                            1-shot
                          </span>
                        )}
                      </td>
                      <td className="mono">
                        {r.event_type_pattern}
                        {r.session_filter && (
                          <span className="badge badge-neutral" style={{ marginLeft: 6 }}>
                            ses:{r.session_filter}
                          </span>
                        )}
                        {r.payload_contains && (
                          <span className="badge badge-neutral" style={{ marginLeft: 6 }}>
                            ~{r.payload_contains}
                          </span>
                        )}
                      </td>
                      <td className="mono">{r.delay_ms > 0 ? `+${r.delay_ms}ms` : '0ms'}</td>
                      <td className="mono" style={{ color: 'var(--text-secondary)' }}>
                        {r.hit_count ?? 0}
                      </td>
                      <td>{r.name}</td>
                      <td style={{ textAlign: 'right' }} onClick={(e) => e.stopPropagation()}>
                        <div className="toolbar-group" style={{ justifyContent: 'flex-end', gap: 4 }}>
                          <button className="btn btn-sm" onClick={() => loadSseRuleIntoForm(r)} title="Edit SSE rule">
                            <Edit3 size={12} />
                          </button>
                          <button
                            className="btn btn-sm"
                            onClick={() => loadSseRuleIntoForm(r, true)}
                            title="Duplicate SSE rule"
                          >
                            <Copy size={12} />
                          </button>
                          <button
                            className="btn btn-sm btn-danger"
                            onClick={() => deleteSseRule(r.id)}
                            title="Delete"
                          >
                            <Trash2 size={12} />
                          </button>
                        </div>
                      </td>
                    </tr>
                  ))
                )}
              </tbody>
            </table>
          </div>
        </div>
      )}

      {subTab === 'openapi' && (
        <div className="split-pane">
          <div className="pane-left" style={{ padding: 14, gap: 12 }}>
            <div className="toolbar-group" style={{ justifyContent: 'space-between' }}>
              <input
                className="input"
                style={{ width: 280 }}
                placeholder="Search OpenAPI paths or SSE event types..."
                value={oaFilter}
                onChange={(e) => setOaFilter(e.target.value)}
              />
              <button className="btn btn-sm" disabled={syncingOa} onClick={syncOpenApi}>
                <RefreshCw size={12} /> {syncingOa ? 'Syncing...' : 'Sync Upstream /doc'}
              </button>
            </div>

            <div className="panel" style={{ padding: 0, overflow: 'hidden' }}>
              <table className="data-table">
                <thead>
                  <tr>
                    <th>Method</th>
                    <th>Endpoint</th>
                    <th>Summary</th>
                    <th></th>
                  </tr>
                </thead>
                <tbody>
                  {filteredOps.map((op) => (
                    <tr key={`${op.method}:${op.path}`} onClick={() => fakeOp(op.method, op.path)}>
                      <td className={`mono method-${op.method}`} style={{ fontWeight: 600 }}>
                        {op.method}
                      </td>
                      <td className="mono">{op.path}</td>
                      <td style={{ color: 'var(--text-secondary)' }}>{op.summary ?? op.operation_id ?? '—'}</td>
                      <td style={{ textAlign: 'right' }}>
                        <button
                          className="btn btn-sm"
                          onClick={(e) => {
                            e.stopPropagation()
                            fakeOp(op.method, op.path)
                          }}
                        >
                          <Wand2 size={11} /> Fake
                        </button>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>

            <div className="panel">
              <div className="panel-title">SSE BusEvent Schemas ({filteredEvents.length})</div>
              <div style={{ display: 'flex', flexWrap: 'wrap', gap: 6 }}>
                {filteredEvents.map((ev) => (
                  <button
                    key={ev.event_type}
                    className="btn btn-sm mono"
                    onClick={() => fakeEv(ev.event_type)}
                  >
                    {ev.event_type}
                  </button>
                ))}
              </div>
            </div>
          </div>

          <aside className="pane-right">
            <div className="inspector-header">
              <span className="mono" style={{ fontWeight: 600 }}>
                {oaPreview ? oaPreview.title : 'Editable Schema Faker Output'}
              </span>
              <div className="toolbar-group">
                {oaPreview?.method && (
                  <button className="btn btn-sm btn-primary" onClick={savePreviewAsMock}>
                    <Save size={12} /> Save as HTTP Rule
                  </button>
                )}
                {oaPreview?.eventType && (
                  <>
                    <button
                      className="btn btn-sm btn-primary"
                      onClick={async () => {
                        try {
                          await api.injectSse({
                            directory: props.activeDirectories[0],
                            payload: JSON.parse(oaEditText),
                            apply_to_shadow: true,
                          })
                          props.onNotify(`Injected edited ${oaPreview.title}`)
                        } catch (e) {
                          props.onNotify(String(e), true)
                        }
                      }}
                    >
                      Inject Now
                    </button>
                    <button className="btn btn-sm" onClick={savePreviewAsSseRule}>
                      Save as SSE Rule
                    </button>
                  </>
                )}
              </div>
            </div>
            <div className="inspector-body">
              {oaPreview ? (
                <>
                  {oaPreview.method && (
                    <div className="form-grid">
                      <div className="field">
                        <label>Path Pattern</label>
                        <input
                          className="input mono"
                          value={oaPathEdit}
                          onChange={(e) => setOaPathEdit(e.target.value)}
                        />
                      </div>
                      <div className="field">
                        <label>HTTP Status</label>
                        <input
                          className="input mono"
                          type="number"
                          value={oaStatus}
                          onChange={(e) => setOaStatus(Number(e.target.value))}
                        />
                      </div>
                    </div>
                  )}
                  <div className="field" style={{ flex: 1 }}>
                    <label>Editable Payload JSON (modify freely before saving or injecting)</label>
                    <textarea
                      className="textarea"
                      rows={18}
                      value={oaEditText}
                      onChange={(e) => setOaEditText(e.target.value)}
                    />
                  </div>
                </>
              ) : (
                <div className="empty-state">
                  Select any REST endpoint or SSE event schema to generate a valid OpenCode payload, then edit it manually before saving or injecting.
                </div>
              )}
            </div>
          </aside>
        </div>
      )}

      {subTab === 'llm' && (
        <div style={{ flex: 1, overflow: 'auto', padding: 14, display: 'flex', flexDirection: 'column', gap: 14 }}>
          <div className="panel">
            <div className="panel-title">
              <span>LLM Scenario &amp; Schema-Validated Mock Generator</span>
              <div className="segmented">
                <button className={llmMode === 'scenario' ? 'active' : ''} onClick={() => setLlmMode('scenario')}>
                  Full SSE Replay Tape
                </button>
                <button className={llmMode === 'http' ? 'active' : ''} onClick={() => setLlmMode('http')}>
                  HTTP Endpoint Mock
                </button>
              </div>
            </div>

            {llmMode === 'scenario' ? (
              <div className="field" style={{ marginBottom: 10 }}>
                <label>Tape Name</label>
                <input className="input" value={llmTapeName} onChange={(e) => setLlmTapeName(e.target.value)} />
              </div>
            ) : (
              <div className="form-grid" style={{ marginBottom: 10 }}>
                <div className="field">
                  <label>Method</label>
                  <select className="select" value={llmHttpMethod} onChange={(e) => setLlmHttpMethod(e.target.value)}>
                    <option value="GET">GET</option>
                    <option value="POST">POST</option>
                    <option value="PUT">PUT</option>
                    <option value="PATCH">PATCH</option>
                    <option value="DELETE">DELETE</option>
                  </select>
                </div>
                <div className="field">
                  <label>Endpoint Path Pattern</label>
                  <input className="input mono" value={llmHttpPath} onChange={(e) => setLlmHttpPath(e.target.value)} />
                </div>
                <div className="field">
                  <label>HTTP Status</label>
                  <input
                    className="input mono"
                    type="number"
                    value={llmHttpStatus}
                    onChange={(e) => setLlmHttpStatus(Number(e.target.value))}
                  />
                </div>
              </div>
            )}

            <div className="field" style={{ marginBottom: 10 }}>
              <label>Natural Language Scenario Prompt</label>
              <textarea
                className="textarea"
                rows={4}
                value={llmPrompt}
                onChange={(e) => setLlmPrompt(e.target.value)}
              />
            </div>

            <div className="toolbar-group">
              <button className="btn btn-primary" disabled={llmBusy} onClick={runLlm}>
                <Sparkles size={13} />
                {llmBusy
                  ? 'Generating & Validating...'
                  : llmMode === 'scenario'
                    ? 'Generate Verified Replay Tape'
                    : 'Generate Verified HTTP Mock'}
              </button>
              <span style={{ fontSize: 11.5, color: 'var(--text-muted)' }}>
                Validated locally via OpenAPI jsonschema + event sequence repair loop. Configure LLM provider under MCP &amp; Config.
              </span>
            </div>
          </div>

          {llmEditBody !== null && (
            <div className="panel">
              <div className="panel-title">
                <span>Editable Generated Output</span>
                {llmMode === 'http' && (
                  <button
                    className="btn btn-sm btn-primary"
                    onClick={async () => {
                      let parsed: unknown = llmEditBody
                      try {
                        parsed = JSON.parse(llmEditBody)
                      } catch {}
                      try {
                        await api.createRule({
                          name: `AI Mock ${llmHttpMethod} ${llmHttpPath}`,
                          method: llmHttpMethod,
                          path_pattern: llmHttpPath.trim(),
                          action_type: 'direct',
                          status: llmHttpStatus,
                          body: parsed,
                        })
                        props.onRefreshRules()
                        props.onNotify(`Saved edited AI mock rule for ${llmHttpMethod} ${llmHttpPath}`)
                      } catch (e) {
                        props.onNotify(String(e), true)
                      }
                    }}
                  >
                    <Save size={12} /> Save Edited Output as HTTP Mock Rule
                  </button>
                )}
              </div>
              {llmMetaNote && (
                <div style={{ fontSize: 11.5, color: 'var(--text-secondary)', marginBottom: 8 }}>{llmMetaNote}</div>
              )}
              <textarea
                className="textarea"
                rows={14}
                value={llmEditBody}
                onChange={(e) => setLlmEditBody(e.target.value)}
              />
            </div>
          )}
        </div>
      )}
    </>
  )
}
