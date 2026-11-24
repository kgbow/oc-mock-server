import { useEffect, useMemo, useState } from 'react'
import { Copy, Edit3, Plus, RotateCcw, Save, Sparkles, Trash2, Wand2, X } from 'lucide-react'
import { api } from '../api'
import type {
  BreakpointPhase,
  HttpTrace,
  HttpTraceSummary,
  MockActionType,
  MockRule,
  MockSseStep,
  OpenApiSummary,
} from '../types'

function formatJson(raw: string | null): string {
  if (!raw) return '(empty)'
  try {
    return JSON.stringify(JSON.parse(raw), null, 2)
  } catch {
    return raw
  }
}

function prettyRawJson(raw: string | null, fallback = '{\n  \n}'): string {
  if (!raw) return fallback
  try {
    return JSON.stringify(JSON.parse(raw), null, 2)
  } catch {
    return raw
  }
}

function parameterizeOpenCodePath(path: string): string {
  return path
    .replace(/\/session\/(?!status\b)([^/]+)/g, '/session/:sessionID')
    .replace(/\/message\/([^/]+)/g, '/message/:messageID')
    .replace(/\/permission\/([^/]+)/g, '/permission/:permissionID')
    .replace(/\/question\/([^/]+)/g, '/question/:questionID')
}

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`
  return `${(n / (1024 * 1024)).toFixed(2)} MB`
}

function statusClass(status: number | null, pending: boolean): string {
  if (pending) return 'badge-info'
  if (!status) return 'badge-err'
  if (status < 300) return 'badge-ok'
  if (status < 400) return 'badge-info'
  if (status < 500) return 'badge-warn'
  return 'badge-err'
}

interface SseDraftItem {
  delay_ms: number
  payloadText: string
  directory: string
  apply_to_shadow: boolean
}

export function NetworkView(props: {
  traces: HttpTraceSummary[]
  rules: MockRule[]
  openapi: OpenApiSummary | null
  activeDirectories: string[]
  onNotify: (msg: string, isErr?: boolean) => void
  onRulesChanged: () => void
}) {
  const [filterText, setFilterText] = useState('')
  const [methodFilter, setMethodFilter] = useState('ALL')
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [detail, setDetail] = useState<HttpTrace | null>(null)
  const [detailTab, setDetailTab] = useState<'response' | 'request' | 'headers' | 'mock'>('response')

  // Manual Mock / Rule Editor state seeded from captured trace
  const [ruleName, setRuleName] = useState('')
  const [ruleMethod, setRuleMethod] = useState('GET')
  const [rulePath, setRulePath] = useState('')
  const [ruleQuery, setRuleQuery] = useState('')
  const [ruleBodyContains, setRuleBodyContains] = useState('')
  const [ruleDirFilter, setRuleDirFilter] = useState('')
  const [ruleAction, setRuleAction] = useState<MockActionType>('direct')
  const [ruleBpPhase, setRuleBpPhase] = useState<BreakpointPhase>('response')
  const [ruleOneShot, setRuleOneShot] = useState(false)
  const [rulePriority, setRulePriority] = useState(50)
  const [ruleStatus, setRuleStatus] = useState(200)
  const [ruleDelayMs, setRuleDelayMs] = useState(0)
  const [ruleHeadersText, setRuleHeadersText] = useState('{\n  "content-type": "application/json"\n}')
  const [ruleBodyText, setRuleBodyText] = useState('{\n  \n}')
  const [sseDrafts, setSseDrafts] = useState<SseDraftItem[]>([])

  // AI Chaos helper that writes directly into the editable box
  const [mutating, setMutating] = useState(false)
  const [mutatePrompt, setMutatePrompt] = useState('Modify fields to simulate an edge case or error payload')

  const filtered = useMemo(() => {
    const q = filterText.trim().toLowerCase()
    return props.traces.filter((t) => {
      if (methodFilter !== 'ALL' && t.method !== methodFilter) return false
      if (!q) return true
      return (
        t.path.toLowerCase().includes(q) ||
        (t.session_id ?? '').toLowerCase().includes(q) ||
        (t.directory ?? '').toLowerCase().includes(q) ||
        String(t.status ?? '').includes(q)
      )
    })
  }, [props.traces, filterText, methodFilter])

  const seedEditorFromTrace = (d: HttpTrace) => {
    setRuleName(`Mock ${d.method} ${d.path}`)
    setRuleMethod(d.method)
    setRulePath(d.path)
    setRuleQuery('')
    setRuleBodyContains('')
    setRuleDirFilter('')
    setRuleAction('direct')
    setRuleBpPhase('response')
    setRuleOneShot(false)
    setRulePriority(50)
    setRuleStatus(d.status ?? 200)
    setRuleDelayMs(0)
    const ct = d.res_headers['content-type'] ?? 'application/json'
    setRuleHeadersText(JSON.stringify({ 'content-type': ct }, null, 2))
    setRuleBodyText(prettyRawJson(d.res_body))
    setSseDrafts([])
  }

  useEffect(() => {
    if (!selectedId) {
      setDetail(null)
      return
    }
    let cancelled = false
    api
      .getHttp(selectedId)
      .then((d) => {
        if (!cancelled) {
          const isNewSelection = detail?.id !== d.id
          setDetail(d)
          if (isNewSelection) {
            seedEditorFromTrace(d)
          }
        }
      })
      .catch(() => {})
    return () => {
      cancelled = true
    }
  }, [selectedId, props.traces])

  const existingSameTarget = useMemo(
    () =>
      props.rules.find(
        (r) => r.method.toUpperCase() === ruleMethod.toUpperCase() && r.path_pattern === rulePath.trim(),
      ) ?? null,
    [props.rules, ruleMethod, rulePath],
  )

  const formatEditorJson = () => {
    try {
      setRuleBodyText(JSON.stringify(JSON.parse(ruleBodyText), null, 2))
    } catch {
      props.onNotify('Body is not valid JSON — will be sent as raw text if saved', true)
    }
  }

  const runMutateIntoEditor = async () => {
    let parsed: unknown
    try {
      parsed = JSON.parse(ruleBodyText)
    } catch {
      props.onNotify('Current body in editor must be valid JSON to run AI Mutate', true)
      return
    }
    setMutating(true)
    try {
      const res = await api.llmMutate({ payload: parsed, instruction: mutatePrompt })
      setRuleBodyText(JSON.stringify(res.data, null, 2))
      setDetailTab('mock')
      props.onNotify('AI mutation written to editor — feel free to tweak manually before saving')
    } catch (e) {
      props.onNotify(String(e), true)
    } finally {
      setMutating(false)
    }
  }

  const addLinkedSseFrame = async (evType = 'session.status') => {
    let defaultPayload: unknown = {
      type: evType,
      properties: {
        sessionID: '{{session_id}}',
        status: { type: 'idle' },
      },
    }
    try {
      const faked = (await api.fakeSse(evType, '{{directory}}')) as Record<string, unknown>
      defaultPayload = JSON.parse(
        JSON.stringify(faked).replace(/"ses_[^"]*"/g, '"{{session_id}}"'),
      )
    } catch {}
    setSseDrafts((prev) => [
      ...prev,
      {
        delay_ms: prev.length === 0 ? 80 : 150,
        payloadText: JSON.stringify(defaultPayload, null, 2),
        directory: '',
        apply_to_shadow: true,
      },
    ])
  }

  const saveRuleFromEditor = async (overwriteId?: string) => {
    if (!rulePath.trim()) {
      props.onNotify('Path pattern cannot be empty', true)
      return
    }
    let headersObj: Record<string, string> = {}
    if (ruleHeadersText.trim()) {
      try {
        headersObj = JSON.parse(ruleHeadersText) as Record<string, string>
      } catch {
        props.onNotify('Response Headers must be a valid JSON object', true)
        return
      }
    }

    let parsedBody: unknown = undefined
    if (ruleAction !== 'breakpoint' && ruleBodyText.trim()) {
      try {
        parsedBody = JSON.parse(ruleBodyText)
      } catch {
        if (ruleAction === 'patch') {
          props.onNotify('JSON Merge Patch requires a valid JSON object', true)
          return
        }
        parsedBody = ruleBodyText
      }
    }

    const sse_sequence: MockSseStep[] = []
    for (let i = 0; i < sseDrafts.length; i++) {
      const item = sseDrafts[i]
      try {
        sse_sequence.push({
          delay_ms: Math.max(0, Number(item.delay_ms) || 0),
          payload: JSON.parse(item.payloadText),
          directory: item.directory.trim() || null,
          apply_to_shadow: item.apply_to_shadow,
        })
      } catch {
        props.onNotify(`Linked SSE frame #${i + 1} is not valid JSON`, true)
        return
      }
    }

    const payload = {
      name: ruleName.trim() || `${ruleMethod} ${rulePath.trim()}`,
      enabled: true,
      priority: rulePriority,
      method: ruleMethod,
      path_pattern: rulePath.trim(),
      query_pattern: ruleQuery.trim() || null,
      body_contains: ruleBodyContains.trim() || null,
      directory_filter: ruleDirFilter.trim() || null,
      action_type: ruleAction,
      breakpoint_phase: ruleBpPhase,
      one_shot: ruleOneShot,
      status: ruleStatus,
      delay_ms: ruleDelayMs,
      headers: headersObj,
      body: ruleAction === 'direct' || ruleAction === 'sse_stream' ? parsedBody : undefined,
      patch: ruleAction === 'patch' ? parsedBody : undefined,
      sse_sequence,
    }

    try {
      if (overwriteId) {
        await api.updateRule(overwriteId, payload)
        props.onNotify(`Updated rule "${payload.name}"`)
      } else {
        await api.createRule(payload)
        props.onNotify(`Saved ${ruleAction} rule for ${ruleMethod} ${rulePath.trim()}`)
      }
      props.onRulesChanged()
    } catch (e) {
      props.onNotify(String(e), true)
    }
  }

  const copyCurl = () => {
    if (!detail) return
    const url = `${detail.path}${detail.query ? `?${detail.query}` : ''}`
    const cmd = `curl -X ${detail.method} "http://127.0.0.1:4097${url}"${
      detail.req_body ? ` -H "Content-Type: application/json" -d '${detail.req_body.replace(/'/g, "'\\''")}'` : ''
    }`
    navigator.clipboard?.writeText(cmd)
    props.onNotify('Copied cURL command')
  }

  return (
    <>
      <div className="toolbar">
        <div className="toolbar-group">
          <input
            className="input"
            style={{ width: 280 }}
            placeholder="Filter path, session_id, directory, status..."
            value={filterText}
            onChange={(e) => setFilterText(e.target.value)}
          />
          <select className="select" value={methodFilter} onChange={(e) => setMethodFilter(e.target.value)}>
            <option value="ALL">All Methods</option>
            <option value="GET">GET</option>
            <option value="POST">POST</option>
            <option value="PUT">PUT</option>
            <option value="PATCH">PATCH</option>
            <option value="DELETE">DELETE</option>
          </select>
        </div>
        <div className="toolbar-group">
          <span className="mono" style={{ color: 'var(--text-muted)', fontSize: 11.5 }}>
            {filtered.length} / {props.traces.length} requests
          </span>
        </div>
      </div>

      <div className="split-pane">
        <div className="pane-left">
          {filtered.length === 0 ? (
            <div className="empty-state">
              No HTTP requests captured yet. Point OpenCode Web/Desktop or <code>opencode attach</code> to the Proxy port.
            </div>
          ) : (
            <table className="data-table">
              <thead>
                <tr>
                  <th>Status</th>
                  <th>Method</th>
                  <th>Path</th>
                  <th>Session</th>
                  <th>Time</th>
                  <th>Size</th>
                  <th>Flags</th>
                </tr>
              </thead>
              <tbody>
                {filtered.map((t) => (
                  <tr
                    key={t.id}
                    className={selectedId === t.id ? 'selected' : ''}
                    onClick={() => setSelectedId(t.id)}
                  >
                    <td>
                      <span className={`badge ${statusClass(t.status, t.pending)}`}>
                        {t.pending ? 'PENDING' : t.status ?? 'ERR'}
                      </span>
                    </td>
                    <td className={`mono method-${t.method}`} style={{ fontWeight: 600 }}>
                      {t.method}
                    </td>
                    <td className="mono" title={`${t.path}${t.query ? `?${t.query}` : ''}`}>
                      {t.path}
                      {t.query ? <span style={{ color: 'var(--text-muted)' }}>?{t.query}</span> : null}
                    </td>
                    <td className="mono" style={{ color: 'var(--text-secondary)' }}>
                      {t.session_id ?? '—'}
                    </td>
                    <td className="mono" style={{ color: 'var(--text-secondary)' }}>
                      {t.duration_ms !== null ? `${t.duration_ms}ms` : '…'}
                    </td>
                    <td className="mono" style={{ color: 'var(--text-muted)' }}>
                      {formatBytes(t.res_size)}
                    </td>
                    <td>
                      {t.mocked && <span className="badge badge-warn">MOCK</span>}
                      {t.upstream_error && <span className="badge badge-err">UPSTREAM ERR</span>}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </div>

        {detail && (
          <aside className="pane-right" style={{ width: '50%', maxWidth: 760 }}>
            <div className="inspector-header">
              <div style={{ minWidth: 0 }}>
                <span className={`mono method-${detail.method}`} style={{ fontWeight: 600, marginRight: 8 }}>
                  {detail.method}
                </span>
                <span className="mono" style={{ fontSize: 12 }}>
                  {detail.path}
                </span>
              </div>
              <div className="toolbar-group">
                <button className="btn btn-sm" onClick={copyCurl} title="Copy cURL">
                  <Copy size={12} /> cURL
                </button>
                <button
                  className="btn btn-sm btn-primary"
                  onClick={() => {
                    seedEditorFromTrace(detail)
                    setDetailTab('mock')
                  }}
                  title="Manually edit status, headers, JSON body, dynamic matchers, or linked SSE events before saving"
                >
                  <Edit3 size={12} /> Edit &amp; Mock
                </button>
                <button className="btn btn-sm btn-ghost" onClick={() => setSelectedId(null)} aria-label="Close">
                  <X size={14} />
                </button>
              </div>
            </div>

            <div className="tab-strip">
              <button
                className={`tab-btn ${detailTab === 'response' ? 'active' : ''}`}
                onClick={() => setDetailTab('response')}
              >
                Response ({formatBytes(detail.res_size)})
              </button>
              <button
                className={`tab-btn ${detailTab === 'request' ? 'active' : ''}`}
                onClick={() => setDetailTab('request')}
              >
                Request ({formatBytes(detail.req_size)})
              </button>
              <button
                className={`tab-btn ${detailTab === 'headers' ? 'active' : ''}`}
                onClick={() => setDetailTab('headers')}
              >
                Headers &amp; Meta
              </button>
              <button
                className={`tab-btn ${detailTab === 'mock' ? 'active' : ''}`}
                onClick={() => setDetailTab('mock')}
              >
                Manual Mock &amp; Breakpoint
              </button>
            </div>

            <div className="inspector-body">
              {detailTab === 'response' && (
                <>
                  {detail.upstream_error && (
                    <div className="badge badge-err" style={{ padding: '6px 10px' }}>
                      {detail.upstream_error}
                    </div>
                  )}
                  <div className="toolbar-group" style={{ justifyContent: 'space-between' }}>
                    <span style={{ fontSize: 11.5, color: 'var(--text-muted)' }}>
                      Captured response body — click <strong>Edit &amp; Mock</strong> to customize manually or attach SSE events.
                    </span>
                    <button
                      className="btn btn-sm"
                      onClick={() => {
                        seedEditorFromTrace(detail)
                        setDetailTab('mock')
                      }}
                    >
                      <Edit3 size={11} /> Customize Response
                    </button>
                  </div>
                  <pre className="code-block">{formatJson(detail.res_body)}</pre>
                </>
              )}

              {detailTab === 'request' && <pre className="code-block">{formatJson(detail.req_body)}</pre>}

              {detailTab === 'headers' && (
                <>
                  <div>
                    <div className="panel-title">Metadata</div>
                    <table className="kv-table">
                      <tbody>
                        <tr>
                          <td>Trace ID</td>
                          <td>{detail.id}</td>
                        </tr>
                        <tr>
                          <td>Status</td>
                          <td>{detail.status ?? 'pending'}</td>
                        </tr>
                        <tr>
                          <td>Duration</td>
                          <td>{detail.duration_ms !== null ? `${detail.duration_ms} ms` : 'pending'}</td>
                        </tr>
                        <tr>
                          <td>Directory</td>
                          <td>{detail.directory ?? '—'}</td>
                        </tr>
                        <tr>
                          <td>Session ID</td>
                          <td>{detail.session_id ?? '—'}</td>
                        </tr>
                        <tr>
                          <td>Mock Rule</td>
                          <td>{detail.mock_rule_id ?? '—'}</td>
                        </tr>
                      </tbody>
                    </table>
                  </div>
                  <div>
                    <div className="panel-title">Request Headers</div>
                    <table className="kv-table">
                      <tbody>
                        {Object.entries(detail.req_headers).map(([k, v]) => (
                          <tr key={k}>
                            <td>{k}</td>
                            <td>{v}</td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  </div>
                  <div>
                    <div className="panel-title">Response Headers</div>
                    <table className="kv-table">
                      <tbody>
                        {Object.entries(detail.res_headers).map(([k, v]) => (
                          <tr key={k}>
                            <td>{k}</td>
                            <td>{v}</td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  </div>
                </>
              )}

              {detailTab === 'mock' && (
                <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
                  <div className="form-grid">
                    <div className="field">
                      <label>Rule Name</label>
                      <input className="input" value={ruleName} onChange={(e) => setRuleName(e.target.value)} />
                    </div>
                    <div className="field">
                      <label>Action Mode</label>
                      <select
                        className="select"
                        value={ruleAction}
                        onChange={(e) => setRuleAction(e.target.value as MockActionType)}
                      >
                        <option value="direct">Direct Mock Response</option>
                        <option value="patch">JSON Merge Patch Upstream</option>
                        <option value="sse_stream">HTTP SSE Stream (text/event-stream)</option>
                        <option value="breakpoint">Dynamic Live Breakpoint</option>
                      </select>
                    </div>
                    <div className="field">
                      <label>Method</label>
                      <select className="select" value={ruleMethod} onChange={(e) => setRuleMethod(e.target.value)}>
                        <option value="ANY">ANY</option>
                        <option value="GET">GET</option>
                        <option value="POST">POST</option>
                        <option value="PUT">PUT</option>
                        <option value="PATCH">PATCH</option>
                        <option value="DELETE">DELETE</option>
                      </select>
                    </div>
                  </div>

                  <div className="field">
                    <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
                      <label>Path Pattern (:param, *, or regex:...)</label>
                      <button
                        className="btn btn-sm btn-ghost"
                        style={{ padding: '1px 6px', fontSize: 11 }}
                        onClick={() => setRulePath(parameterizeOpenCodePath(rulePath))}
                      >
                        <Wand2 size={11} /> Wildcard :sessionID
                      </button>
                    </div>
                    <input
                      className="input mono"
                      value={rulePath}
                      onChange={(e) => setRulePath(e.target.value)}
                      placeholder="/session/:sessionID/message or regex:^/session/.*"
                    />
                  </div>

                  <div className="form-grid">
                    <div className="field">
                      <label>Query Filter (substring / * / regex:)</label>
                      <input
                        className="input mono"
                        value={ruleQuery}
                        onChange={(e) => setRuleQuery(e.target.value)}
                        placeholder={detail.query ?? 'Optional query match'}
                      />
                    </div>
                    <div className="field">
                      <label>Req Body Contains (* / regex:)</label>
                      <input
                        className="input mono"
                        value={ruleBodyContains}
                        onChange={(e) => setRuleBodyContains(e.target.value)}
                        placeholder="Optional body keyword"
                      />
                    </div>
                    <div className="field">
                      <label>Directory Filter</label>
                      <input
                        className="input mono"
                        value={ruleDirFilter}
                        onChange={(e) => setRuleDirFilter(e.target.value)}
                        placeholder="Any directory"
                      />
                    </div>
                  </div>

                  <div className="form-grid">
                    {ruleAction === 'breakpoint' ? (
                      <div className="field">
                        <label>Breakpoint Stage</label>
                        <select
                          className="select"
                          value={ruleBpPhase}
                          onChange={(e) => setRuleBpPhase(e.target.value as BreakpointPhase)}
                        >
                          <option value="response">Response (Intercept &amp; Edit Real Upstream JSON)</option>
                          <option value="request">Request (Before Upstream)</option>
                          <option value="both">Both Request &amp; Response</option>
                        </select>
                      </div>
                    ) : (
                      <div className="field">
                        <label>HTTP Status</label>
                        <input
                          className="input mono"
                          type="number"
                          value={ruleStatus}
                          onChange={(e) => setRuleStatus(Number(e.target.value))}
                        />
                      </div>
                    )}
                    <div className="field">
                      <label>Delay (ms)</label>
                      <input
                        className="input mono"
                        type="number"
                        value={ruleDelayMs}
                        onChange={(e) => setRuleDelayMs(Number(e.target.value))}
                      />
                    </div>
                    <div className="field">
                      <label>Priority (lower runs first)</label>
                      <input
                        className="input mono"
                        type="number"
                        value={rulePriority}
                        onChange={(e) => setRulePriority(Number(e.target.value))}
                      />
                    </div>
                  </div>

                  <label style={{ display: 'inline-flex', alignItems: 'center', gap: 6, fontSize: 12, color: 'var(--text-secondary)' }}>
                    <input
                      type="checkbox"
                      checked={ruleOneShot}
                      onChange={(e) => setRuleOneShot(e.target.checked)}
                    />
                    One-shot rule (automatically disable after matching once)
                  </label>

                  {ruleAction !== 'breakpoint' && (
                    <>
                      <div className="field">
                        <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
                          <label>
                            {ruleAction === 'patch'
                              ? 'JSON Merge Patch (applied onto upstream response)'
                              : 'Editable Mock Response Body (JSON or text)'}
                          </label>
                          <div className="toolbar-group" style={{ gap: 4 }}>
                            <button className="btn btn-sm" onClick={formatEditorJson}>
                              Format JSON
                            </button>
                            <button
                              className="btn btn-sm"
                              onClick={() => setRuleBodyText(prettyRawJson(detail.res_body))}
                              title="Reset to captured response body"
                            >
                              <RotateCcw size={11} /> Reset
                            </button>
                          </div>
                        </div>
                        <textarea
                          className="textarea"
                          rows={9}
                          value={ruleBodyText}
                          onChange={(e) => setRuleBodyText(e.target.value)}
                        />
                      </div>

                      <div className="panel" style={{ padding: 10 }}>
                        <div style={{ fontSize: 11.5, color: 'var(--text-secondary)', marginBottom: 6 }}>
                          Optional AI Helper (mutates editor JSON above so you can tweak manually)
                        </div>
                        <div style={{ display: 'flex', gap: 6 }}>
                          <input
                            className="input"
                            style={{ flex: 1 }}
                            value={mutatePrompt}
                            onChange={(e) => setMutatePrompt(e.target.value)}
                          />
                          <button className="btn btn-sm" disabled={mutating} onClick={runMutateIntoEditor}>
                            <Sparkles size={12} /> {mutating ? 'Mutating...' : 'AI Mutate Editor'}
                          </button>
                        </div>
                      </div>

                      <div className="field">
                        <label>Custom Response Headers (JSON)</label>
                        <textarea
                          className="textarea"
                          rows={2}
                          value={ruleHeadersText}
                          onChange={(e) => setRuleHeadersText(e.target.value)}
                        />
                      </div>

                      <div className="panel" style={{ padding: 10 }}>
                        <div className="panel-title" style={{ marginBottom: 6, fontSize: 12 }}>
                          <span>
                            {ruleAction === 'sse_stream'
                              ? 'Streamed SSE Chunks (returned on this HTTP connection)'
                              : 'HTTP-Triggered SSE Hub Sequence (pushed to /global/event & /event)'}
                          </span>
                          <button className="btn btn-sm" onClick={() => addLinkedSseFrame('session.status')}>
                            <Plus size={11} /> Add SSE Step
                          </button>
                        </div>
                        {sseDrafts.length === 0 ? (
                          <div style={{ fontSize: 11.5, color: 'var(--text-muted)' }}>
                            No linked SSE events. Add steps to push async SSE frames (supports <code>{'{{session_id}}'}</code> &amp; <code>{'{{directory}}'}</code>) right after this HTTP mock responds.
                          </div>
                        ) : (
                          <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
                            {sseDrafts.map((item, idx) => (
                              <div
                                key={idx}
                                style={{
                                  border: '1px solid var(--border-subtle)',
                                  borderRadius: 6,
                                  padding: 8,
                                  background: 'var(--bg-inset)',
                                }}
                              >
                                <div className="toolbar-group" style={{ justifyContent: 'space-between', marginBottom: 6 }}>
                                  <div className="toolbar-group">
                                    <span className="badge badge-info">#{idx + 1}</span>
                                    <label style={{ fontSize: 11, color: 'var(--text-secondary)' }}>+Delay (ms):</label>
                                    <input
                                      className="input mono"
                                      style={{ width: 76 }}
                                      type="number"
                                      value={item.delay_ms}
                                      onChange={(e) => {
                                        const v = Number(e.target.value)
                                        setSseDrafts((prev) =>
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
                                          setSseDrafts((prev) =>
                                            prev.map((s, i) => (i === idx ? { ...s, payloadText: text } : s)),
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
                                    onClick={() => setSseDrafts((prev) => prev.filter((_, i) => i !== idx))}
                                  >
                                    <Trash2 size={11} />
                                  </button>
                                </div>
                                <textarea
                                  className="textarea"
                                  rows={4}
                                  value={item.payloadText}
                                  onChange={(e) => {
                                    const val = e.target.value
                                    setSseDrafts((prev) =>
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

                  <div className="toolbar-group" style={{ paddingTop: 4 }}>
                    <button className="btn btn-primary" onClick={() => saveRuleFromEditor()}>
                      <Save size={13} /> Save as New Rule
                    </button>
                    {existingSameTarget && (
                      <button className="btn btn-warn" onClick={() => saveRuleFromEditor(existingSameTarget.id)}>
                        Update Existing "{existingSameTarget.name}"
                      </button>
                    )}
                  </div>
                </div>
              )}
            </div>
          </aside>
        )}
      </div>
    </>
  )
}
