import { useEffect, useMemo, useState } from 'react'
import { Copy, Sparkles, Wand2, X } from 'lucide-react'
import { api } from '../api'
import type { HttpTrace, HttpTraceSummary } from '../types'

function formatJson(raw: string | null): string {
  if (!raw) return '(empty)'
  try {
    return JSON.stringify(JSON.parse(raw), null, 2)
  } catch {
    return raw
  }
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

export function NetworkView(props: {
  traces: HttpTraceSummary[]
  onNotify: (msg: string, isErr?: boolean) => void
  onRulesChanged: () => void
}) {
  const [filterText, setFilterText] = useState('')
  const [methodFilter, setMethodFilter] = useState('ALL')
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [detail, setDetail] = useState<HttpTrace | null>(null)
  const [detailTab, setDetailTab] = useState<'response' | 'request' | 'headers'>('response')
  const [mutating, setMutating] = useState(false)
  const [mutatePrompt, setMutatePrompt] = useState('Inject a 500 error or malformed tool part to test UI resilience')
  const [mutatedOutput, setMutatedOutput] = useState<string | null>(null)

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
          setDetail(d)
          setMutatedOutput(null)
        }
      })
      .catch(() => {})
    return () => {
      cancelled = true
    }
  }, [selectedId, props.traces])

  const createMockFromTrace = async (bodyOverride?: unknown) => {
    if (!detail) return
    let bodyVal: unknown = bodyOverride
    if (bodyVal === undefined && detail.res_body) {
      try {
        bodyVal = JSON.parse(detail.res_body)
      } catch {
        bodyVal = detail.res_body
      }
    }
    try {
      await api.createRule({
        name: `Mock ${detail.method} ${detail.path}`,
        method: detail.method,
        path_pattern: detail.path,
        action_type: 'direct',
        status: detail.status ?? 200,
        body: bodyVal ?? {},
      })
      props.onRulesChanged()
      props.onNotify(`Saved mock rule for ${detail.method} ${detail.path}`)
    } catch (e) {
      props.onNotify(String(e), true)
    }
  }

  const runMutate = async () => {
    if (!detail?.res_body) return
    let parsed: unknown
    try {
      parsed = JSON.parse(detail.res_body)
    } catch {
      props.onNotify('Response body is not valid JSON', true)
      return
    }
    setMutating(true)
    try {
      const res = await api.llmMutate({ payload: parsed, instruction: mutatePrompt })
      setMutatedOutput(JSON.stringify(res.data, null, 2))
      props.onNotify('AI mutation generated — click Save Variant as Mock to activate')
    } catch (e) {
      props.onNotify(String(e), true)
    } finally {
      setMutating(false)
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
          <aside className="pane-right">
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
                <button className="btn btn-sm btn-primary" onClick={() => createMockFromTrace()}>
                  <Wand2 size={12} /> Save Mock
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
            </div>

            <div className="inspector-body">
              {detailTab === 'response' && (
                <>
                  {detail.upstream_error && (
                    <div className="badge badge-err" style={{ padding: '6px 10px' }}>
                      {detail.upstream_error}
                    </div>
                  )}
                  <pre className="code-block">{formatJson(detail.res_body)}</pre>

                  {detail.res_body && (
                    <div className="panel" style={{ padding: 10 }}>
                      <div className="panel-title" style={{ marginBottom: 6, fontSize: 12 }}>
                        <span>AI Chaos Mutation</span>
                      </div>
                      <div style={{ display: 'flex', gap: 6 }}>
                        <input
                          className="input"
                          style={{ flex: 1 }}
                          value={mutatePrompt}
                          onChange={(e) => setMutatePrompt(e.target.value)}
                        />
                        <button className="btn btn-sm" disabled={mutating} onClick={runMutate}>
                          <Sparkles size={12} /> {mutating ? 'Mutating...' : 'Mutate'}
                        </button>
                      </div>
                      {mutatedOutput && (
                        <div style={{ marginTop: 8, display: 'flex', flexDirection: 'column', gap: 6 }}>
                          <pre className="code-block" style={{ maxHeight: 200 }}>
                            {mutatedOutput}
                          </pre>
                          <button
                            className="btn btn-sm btn-primary"
                            onClick={() => createMockFromTrace(JSON.parse(mutatedOutput))}
                          >
                            Save Mutated Variant as Mock Rule
                          </button>
                        </div>
                      )}
                    </div>
                  )}
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
            </div>
          </aside>
        )}
      </div>
    </>
  )
}
