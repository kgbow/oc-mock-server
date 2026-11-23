import { useState } from 'react'
import { Plus, RefreshCw, Sparkles, Trash2, Wand2 } from 'lucide-react'
import { api } from '../api'
import type { MockRule, OpenApiSummary } from '../types'

export function MockStudioView(props: {
  rules: MockRule[]
  openapi: OpenApiSummary | null
  activeDirectories: string[]
  onRefreshRules: () => void
  onRefreshOpenApi: () => void
  onRefreshTapes: () => void
  onNotify: (msg: string, isErr?: boolean) => void
}) {
  const [subTab, setSubTab] = useState<'rules' | 'openapi' | 'llm'>('rules')

  // Rule creator form
  const [name, setName] = useState('Mock session status')
  const [method, setMethod] = useState('GET')
  const [pathPattern, setPathPattern] = useState('/session/status')
  const [actionType, setActionType] = useState<'direct' | 'patch' | 'breakpoint'>('direct')
  const [status, setStatus] = useState(200)
  const [delayMs, setDelayMs] = useState(0)
  const [dirFilter, setDirFilter] = useState('')
  const [payloadText, setPayloadText] = useState('{\n  "ses_mock_001": { "type": "busy" }\n}')

  // OpenAPI explorer state
  const [oaFilter, setOaFilter] = useState('')
  const [oaPreview, setOaPreview] = useState<{ title: string; method?: string; path?: string; data: unknown } | null>(null)
  const [syncingOa, setSyncingOa] = useState(false)

  // LLM Studio state
  const [llmMode, setLlmMode] = useState<'scenario' | 'http'>('scenario')
  const [llmPrompt, setLlmPrompt] = useState(
    'Generate a realistic OpenCode session where the assistant reasons for 2 steps, runs a bash test command that fails once, asks for permission to edit src/main.rs, and streams a concise summary with a file diff before going idle.',
  )
  const [llmHttpMethod, setLlmHttpMethod] = useState('GET')
  const [llmHttpPath, setLlmHttpPath] = useState('/session/:sessionID/message')
  const [llmTapeName, setLlmTapeName] = useState('AI Scenario Replay')
  const [llmBusy, setLlmBusy] = useState(false)
  const [llmResult, setLlmResult] = useState<string | null>(null)

  const handleCreateRule = async () => {
    let parsed: unknown = undefined
    if (actionType !== 'breakpoint' && payloadText.trim()) {
      try {
        parsed = JSON.parse(payloadText)
      } catch {
        parsed = payloadText
      }
    }
    try {
      await api.createRule({
        name: name.trim() || `${method} ${pathPattern}`,
        method,
        path_pattern: pathPattern.trim(),
        directory_filter: dirFilter.trim() || null,
        action_type: actionType,
        status,
        delay_ms: delayMs,
        body: actionType === 'direct' ? parsed : undefined,
        patch: actionType === 'patch' ? parsed : undefined,
      })
      props.onRefreshRules()
      props.onNotify(`Created ${actionType} rule for ${method} ${pathPattern}`)
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
        directory_filter: r.directory_filter,
        action_type: r.action_type,
        status: r.status,
        delay_ms: r.delay_ms,
        headers: r.headers,
        body: r.body,
        patch: r.patch,
      })
      props.onRefreshRules()
    } catch (e) {
      props.onNotify(String(e), true)
    }
  }

  const deleteRule = async (id: string) => {
    try {
      await api.deleteRule(id)
      props.onRefreshRules()
      props.onNotify('Deleted mock rule')
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
      setOaPreview({ title: `${m} ${p}`, method: m, path: p, data })
    } catch (e) {
      props.onNotify(String(e), true)
    }
  }

  const fakeEv = async (evType: string) => {
    try {
      const data = await api.fakeSse(evType, props.activeDirectories[0])
      setOaPreview({ title: `SSE ${evType}`, data })
    } catch (e) {
      props.onNotify(String(e), true)
    }
  }

  const savePreviewAsMock = async () => {
    if (!oaPreview?.method || !oaPreview?.path) return
    try {
      await api.createRule({
        name: `Faked ${oaPreview.method} ${oaPreview.path}`,
        method: oaPreview.method,
        path_pattern: oaPreview.path,
        action_type: 'direct',
        status: 200,
        body: oaPreview.data,
      })
      props.onRefreshRules()
      props.onNotify(`Saved schema-generated mock for ${oaPreview.method} ${oaPreview.path}`)
    } catch (e) {
      props.onNotify(String(e), true)
    }
  }

  const runLlm = async () => {
    setLlmBusy(true)
    setLlmResult(null)
    try {
      if (llmMode === 'scenario') {
        const res = await api.llmGenerateScenario({
          prompt: llmPrompt,
          directory: props.activeDirectories[0],
          tape_name: llmTapeName,
          save_tape: true,
        })
        props.onRefreshTapes()
        setLlmResult(JSON.stringify(res, null, 2))
        props.onNotify(
          `Generated & saved tape "${res.tape.name}" (${res.validated ? 'schema-verified' : 'with warnings'}, ${res.attempts} attempt(s))`,
        )
      } else {
        const res = await api.llmGenerateHttp({
          method: llmHttpMethod,
          path: llmHttpPath,
          prompt: llmPrompt,
          directory: props.activeDirectories[0],
        })
        setLlmResult(JSON.stringify(res, null, 2))
        props.onNotify(
          `Generated HTTP mock (${res.validated ? 'schema-verified' : 'with warnings'}, ${res.attempts} attempt(s))`,
        )
      }
    } catch (e) {
      props.onNotify(String(e), true)
    } finally {
      setLlmBusy(false)
    }
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
          Mock Rules ({props.rules.length})
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
            <div className="panel-title">Create Mock / Patch / Breakpoint Rule</div>
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
                <label>Path Pattern (:param &amp; * supported)</label>
                <input
                  className="input mono"
                  value={pathPattern}
                  onChange={(e) => setPathPattern(e.target.value)}
                  placeholder="/session/:sessionID/message"
                />
              </div>
              <div className="field">
                <label>Action</label>
                <select
                  className="select"
                  value={actionType}
                  onChange={(e) => setActionType(e.target.value as 'direct' | 'patch' | 'breakpoint')}
                >
                  <option value="direct">Direct Mock Response</option>
                  <option value="patch">JSON Merge Patch Upstream</option>
                  <option value="breakpoint">Suspend Breakpoint</option>
                </select>
              </div>
              <div className="field">
                <label>HTTP Status</label>
                <input
                  className="input mono"
                  type="number"
                  value={status}
                  onChange={(e) => setStatus(Number(e.target.value))}
                />
              </div>
              <div className="field">
                <label>Artificial Delay (ms)</label>
                <input
                  className="input mono"
                  type="number"
                  value={delayMs}
                  onChange={(e) => setDelayMs(Number(e.target.value))}
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
            </div>

            {actionType !== 'breakpoint' && (
              <div className="field" style={{ marginBottom: 10 }}>
                <label>{actionType === 'direct' ? 'Mock JSON Body' : 'JSON Merge Patch Object'}</label>
                <textarea
                  className="textarea"
                  rows={5}
                  value={payloadText}
                  onChange={(e) => setPayloadText(e.target.value)}
                />
              </div>
            )}

            <div className="toolbar-group">
              <button className="btn btn-primary" onClick={handleCreateRule}>
                <Plus size={13} /> Add Rule
              </button>
              <button
                className="btn"
                onClick={async () => {
                  try {
                    const fake = await api.fakeHttp(method === 'ANY' ? 'GET' : method, pathPattern, props.activeDirectories[0])
                    setPayloadText(JSON.stringify(fake, null, 2))
                    props.onNotify('Filled body using OpenAPI Schema Faker')
                  } catch (e) {
                    props.onNotify(String(e), true)
                  }
                }}
              >
                <Wand2 size={13} /> Auto-fill from OpenAPI
              </button>
            </div>
          </div>

          <div className="panel" style={{ padding: 0, overflow: 'hidden' }}>
            <table className="data-table">
              <thead>
                <tr>
                  <th>State</th>
                  <th>Action</th>
                  <th>Method</th>
                  <th>Pattern</th>
                  <th>Status / Delay</th>
                  <th>Name</th>
                  <th></th>
                </tr>
              </thead>
              <tbody>
                {props.rules.length === 0 ? (
                  <tr>
                    <td colSpan={7} className="empty-state">
                      No mock rules configured.
                    </td>
                  </tr>
                ) : (
                  props.rules.map((r) => (
                    <tr key={r.id}>
                      <td>
                        <button
                          className={`btn btn-sm ${r.enabled ? 'btn-primary' : ''}`}
                          onClick={() => toggleRule(r)}
                        >
                          {r.enabled ? 'ON' : 'OFF'}
                        </button>
                      </td>
                      <td>
                        <span
                          className={`badge ${
                            r.action_type === 'breakpoint'
                              ? 'badge-err'
                              : r.action_type === 'patch'
                                ? 'badge-info'
                                : 'badge-warn'
                          }`}
                        >
                          {r.action_type}
                        </span>
                      </td>
                      <td className={`mono method-${r.method}`} style={{ fontWeight: 600 }}>
                        {r.method}
                      </td>
                      <td className="mono">{r.path_pattern}</td>
                      <td className="mono">
                        {r.status} {r.delay_ms > 0 ? `+${r.delay_ms}ms` : ''}
                      </td>
                      <td>{r.name}</td>
                      <td style={{ textAlign: 'right' }}>
                        <button className="btn btn-sm btn-danger" onClick={() => deleteRule(r.id)}>
                          <Trash2 size={12} />
                        </button>
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
                {oaPreview ? oaPreview.title : 'Schema Faker Output'}
              </span>
              {oaPreview?.method && (
                <button className="btn btn-sm btn-primary" onClick={savePreviewAsMock}>
                  Save as Mock Rule
                </button>
              )}
              {oaPreview && !oaPreview.method && (
                <button
                  className="btn btn-sm btn-primary"
                  onClick={async () => {
                    try {
                      await api.injectSse({
                        directory: props.activeDirectories[0],
                        payload: oaPreview.data,
                        apply_to_shadow: true,
                      })
                      props.onNotify(`Injected ${oaPreview.title}`)
                    } catch (e) {
                      props.onNotify(String(e), true)
                    }
                  }}
                >
                  Inject Event to Clients
                </button>
              )}
            </div>
            <div className="inspector-body">
              {oaPreview ? (
                <pre className="code-block">{JSON.stringify(oaPreview.data, null, 2)}</pre>
              ) : (
                <div className="empty-state">
                  Select any REST endpoint or SSE event schema to deterministically generate a valid OpenCode payload.
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
                    <option value="PATCH">PATCH</option>
                  </select>
                </div>
                <div className="field">
                  <label>Endpoint Path</label>
                  <input className="input mono" value={llmHttpPath} onChange={(e) => setLlmHttpPath(e.target.value)} />
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

          {llmResult && (
            <div className="panel">
              <div className="panel-title">Generated Result</div>
              <pre className="code-block" style={{ maxHeight: 420 }}>
                {llmResult}
              </pre>
            </div>
          )}
        </div>
      )}
    </>
  )
}
