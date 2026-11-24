import { useEffect, useState } from 'react'
import { OctagonX, Play, Plus, Save, Send, Trash2 } from 'lucide-react'
import { api } from '../api'
import type {
  BreakpointPhase,
  MockRule,
  OpenApiSummary,
  PendingBreakpoint,
  SseMockRule,
} from '../types'

function prettyText(raw: string | null | undefined, fallback = ''): string {
  if (!raw) return fallback
  try {
    return JSON.stringify(JSON.parse(raw), null, 2)
  } catch {
    return raw
  }
}

export function BreakpointsView(props: {
  breakpoints: PendingBreakpoint[]
  rules: MockRule[]
  sseRules: SseMockRule[]
  openapi: OpenApiSummary | null
  onRefresh: () => void
  onRefreshRules: () => void
  onRefreshSseRules: () => void
  onNotify: (msg: string, isErr?: boolean) => void
}) {
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const selected = props.breakpoints.find((b) => b.id === selectedId) ?? props.breakpoints[0] ?? null

  // Quick dynamic breakpoint creator state
  const [quickKind, setQuickKind] = useState<'http' | 'sse'>('http')
  const [quickMethod, setQuickMethod] = useState('ANY')
  const [quickPattern, setQuickPattern] = useState('/session/*')
  const [quickPhase, setQuickPhase] = useState<BreakpointPhase>('response')
  const [quickFilter, setQuickFilter] = useState('')
  const [quickOneShot, setQuickOneShot] = useState(true)

  // HTTP Request editor fields
  const [editMethod, setEditMethod] = useState('GET')
  const [editPath, setEditPath] = useState('')
  const [editQuery, setEditQuery] = useState('')
  const [reqHeadersText, setReqHeadersText] = useState('{}')
  const [reqBody, setReqBody] = useState('')

  // HTTP Response / Direct Mock editor fields
  const [resStatus, setResStatus] = useState(200)
  const [resHeadersText, setResHeadersText] = useState('{\n  "content-type": "application/json"\n}')
  const [resBody, setResBody] = useState('{\n  "ok": true\n}')

  // SSE Breakpoint editor fields
  const [ssePayloadText, setSsePayloadText] = useState('{}')
  const [sseDir, setSseDir] = useState('')
  const [sseShadow, setSseShadow] = useState(false)

  useEffect(() => {
    if (!selected) return
    setEditMethod(selected.method)
    setEditPath(selected.path)
    setEditQuery(selected.query ?? '')
    setReqHeadersText(JSON.stringify(selected.req_headers ?? {}, null, 2))
    setReqBody(prettyText(selected.req_body, ''))

    if (selected.kind === 'http_response') {
      setResStatus(selected.res_status ?? 200)
      setResHeadersText(JSON.stringify(selected.res_headers ?? { 'content-type': 'application/json' }, null, 2))
      setResBody(prettyText(selected.res_body, '{}'))
    } else if (selected.kind === 'sse_event') {
      setSsePayloadText(JSON.stringify(selected.sse_payload ?? {}, null, 2))
      setSseDir(selected.directory ?? '')
      setSseShadow(false)
    } else {
      setResStatus(200)
      setResHeadersText('{\n  "content-type": "application/json"\n}')
      setResBody('{\n  "ok": true\n}')
    }
  }, [selected?.id])

  const activeHttpBpRules = props.rules.filter((r) => r.action_type === 'breakpoint')
  const activeSseBpRules = props.sseRules.filter((r) => r.action_type === 'breakpoint')

  const createQuickBreakpoint = async () => {
    if (!quickPattern.trim()) {
      props.onNotify('Pattern cannot be empty', true)
      return
    }
    try {
      if (quickKind === 'http') {
        await api.createRule({
          name: `BP ${quickMethod} ${quickPattern.trim()}`,
          method: quickMethod,
          path_pattern: quickPattern.trim(),
          body_contains: quickFilter.trim() || null,
          action_type: 'breakpoint',
          breakpoint_phase: quickPhase,
          priority: 10,
          one_shot: quickOneShot,
        })
        props.onRefreshRules()
        props.onNotify(`Added dynamic HTTP ${quickPhase} breakpoint on ${quickMethod} ${quickPattern.trim()}`)
      } else {
        await api.createSseRule({
          name: `SSE BP ${quickPattern.trim()}`,
          event_type_pattern: quickPattern.trim(),
          payload_contains: quickFilter.trim() || null,
          action_type: 'breakpoint',
          priority: 10,
          one_shot: quickOneShot,
        })
        props.onRefreshSseRules()
        props.onNotify(`Added dynamic SSE breakpoint on ${quickPattern.trim()}`)
      }
    } catch (e) {
      props.onNotify(String(e), true)
    }
  }

  const forwardRequest = async () => {
    if (!selected) return
    let parsedHeaders: Record<string, string> | null = null
    if (reqHeadersText.trim()) {
      try {
        parsedHeaders = JSON.parse(reqHeadersText) as Record<string, string>
      } catch {
        props.onNotify('Request Headers JSON is invalid', true)
        return
      }
    }
    try {
      await api.resolveBreakpoint(selected.id, {
        decision: 'forward',
        method: editMethod,
        path: editPath,
        query: editQuery,
        req_body: reqBody,
        req_headers: parsedHeaders,
      })
      props.onRefresh()
      props.onNotify(`Forwarded ${editMethod} ${editPath} to upstream`)
    } catch (e) {
      props.onNotify(String(e), true)
    }
  }

  const respondHttp = async (alsoSaveRule = false) => {
    if (!selected) return
    let parsedHeaders: Record<string, string> = { 'content-type': 'application/json' }
    if (resHeadersText.trim()) {
      try {
        parsedHeaders = JSON.parse(resHeadersText) as Record<string, string>
      } catch {
        props.onNotify('Response Headers JSON is invalid', true)
        return
      }
    }
    try {
      if (alsoSaveRule) {
        let bodyVal: unknown = resBody
        try {
          bodyVal = JSON.parse(resBody)
        } catch {}
        await api.createRule({
          name: `Saved from BP ${selected.method} ${selected.path}`,
          method: selected.method,
          path_pattern: selected.path,
          action_type: 'direct',
          priority: 20,
          status: resStatus,
          headers: parsedHeaders,
          body: bodyVal,
        })
        props.onRefreshRules()
      }
      await api.resolveBreakpoint(selected.id, {
        decision: 'respond',
        status: resStatus,
        headers: parsedHeaders,
        body: resBody,
      })
      props.onRefresh()
      props.onNotify(
        alsoSaveRule
          ? `Saved persistent mock rule & resumed with HTTP ${resStatus}`
          : `Returned edited HTTP ${resStatus} response to client`,
      )
    } catch (e) {
      props.onNotify(String(e), true)
    }
  }

  const emitEditedSse = async (alsoSaveRule = false) => {
    if (!selected) return
    let parsedPayload: unknown
    try {
      parsedPayload = JSON.parse(ssePayloadText)
    } catch {
      props.onNotify('SSE Payload must be valid JSON', true)
      return
    }
    try {
      if (alsoSaveRule && selected.sse_event_type) {
        await api.createSseRule({
          name: `Saved from BP ${selected.sse_event_type}`,
          event_type_pattern: selected.sse_event_type,
          action_type: 'replace',
          priority: 20,
          replace_events: [{ delay_ms: 0, payload: parsedPayload, apply_to_shadow: sseShadow }],
        })
        props.onRefreshSseRules()
      }
      await api.resolveBreakpoint(selected.id, {
        decision: 'emit_sse',
        payload: parsedPayload,
        directory: sseDir.trim() || null,
        apply_to_shadow: sseShadow,
      })
      props.onRefresh()
      props.onNotify('Forwarded edited SSE frame to connected clients')
    } catch (e) {
      props.onNotify(String(e), true)
    }
  }

  const dropSseFrame = async () => {
    if (!selected) return
    try {
      await api.resolveBreakpoint(selected.id, { decision: 'drop' })
      props.onRefresh()
      props.onNotify(`Dropped SSE frame (${selected.path})`)
    } catch (e) {
      props.onNotify(String(e), true)
    }
  }

  const abortHttp = async () => {
    if (!selected) return
    try {
      await api.resolveBreakpoint(selected.id, {
        decision: 'abort',
        message: 'Aborted from oc-mock-server DevTools breakpoint',
      })
      props.onRefresh()
      props.onNotify('Aborted request with HTTP 502')
    } catch (e) {
      props.onNotify(String(e), true)
    }
  }

  return (
    <>
      <div
        style={{
          padding: '10px 14px',
          borderBottom: '1px solid var(--border-subtle)',
          background: 'var(--bg-surface)',
          display: 'flex',
          flexDirection: 'column',
          gap: 8,
        }}
      >
        <div className="toolbar-group" style={{ justifyContent: 'space-between' }}>
          <div className="toolbar-group">
            <div className="segmented">
              <button
                className={quickKind === 'http' ? 'active' : ''}
                onClick={() => {
                  setQuickKind('http')
                  setQuickPattern('/session/*')
                }}
              >
                HTTP Breakpoint
              </button>
              <button
                className={quickKind === 'sse' ? 'active' : ''}
                onClick={() => {
                  setQuickKind('sse')
                  setQuickPattern('permission.asked')
                }}
              >
                SSE Breakpoint
              </button>
            </div>

            {quickKind === 'http' && (
              <select className="select" value={quickMethod} onChange={(e) => setQuickMethod(e.target.value)}>
                <option value="ANY">ANY</option>
                <option value="GET">GET</option>
                <option value="POST">POST</option>
                <option value="PUT">PUT</option>
                <option value="PATCH">PATCH</option>
                <option value="DELETE">DELETE</option>
              </select>
            )}

            <input
              className="input mono"
              style={{ width: 220 }}
              list="quick-bp-patterns"
              value={quickPattern}
              onChange={(e) => setQuickPattern(e.target.value)}
              placeholder={
                quickKind === 'http'
                  ? '/session/:id/message or regex:...'
                  : 'permission.asked or message.part.*'
              }
            />
            <datalist id="quick-bp-patterns">
              {quickKind === 'sse' &&
                (props.openapi?.events ?? []).map((ev) => (
                  <option key={ev.event_type} value={ev.event_type} />
                ))}
            </datalist>

            {quickKind === 'http' && (
              <select
                className="select"
                value={quickPhase}
                onChange={(e) => setQuickPhase(e.target.value as BreakpointPhase)}
              >
                <option value="response">Intercept Real Response</option>
                <option value="request">Intercept Request</option>
                <option value="both">Both Req &amp; Res</option>
              </select>
            )}

            <input
              className="input mono"
              style={{ width: 170 }}
              value={quickFilter}
              onChange={(e) => setQuickFilter(e.target.value)}
              placeholder="Body/Payload contains..."
            />

            <label style={{ display: 'inline-flex', alignItems: 'center', gap: 5, fontSize: 11.5 }}>
              <input
                type="checkbox"
                checked={quickOneShot}
                onChange={(e) => setQuickOneShot(e.target.checked)}
              />
              1-shot
            </label>

            <button className="btn btn-sm btn-primary" onClick={createQuickBreakpoint}>
              <Plus size={12} /> Watch
            </button>
          </div>
        </div>

        {(activeHttpBpRules.length > 0 || activeSseBpRules.length > 0) && (
          <div style={{ display: 'flex', flexWrap: 'wrap', gap: 6, alignItems: 'center' }}>
            <span style={{ fontSize: 11, color: 'var(--text-muted)' }}>Watchers:</span>
            {activeHttpBpRules.map((r) => (
              <span key={r.id} className={`badge ${r.enabled ? 'badge-warn' : 'badge-neutral'}`} style={{ gap: 6 }}>
                <span
                  style={{ cursor: 'pointer' }}
                  onClick={async () => {
                    await api.updateRule(r.id, { ...r, enabled: !r.enabled })
                    props.onRefreshRules()
                  }}
                >
                  [{r.enabled ? 'ON' : 'OFF'}] HTTP({r.breakpoint_phase}) {r.method} {r.path_pattern}
                </span>
                <Trash2
                  size={10}
                  style={{ cursor: 'pointer' }}
                  onClick={async () => {
                    await api.deleteRule(r.id)
                    props.onRefreshRules()
                  }}
                />
              </span>
            ))}
            {activeSseBpRules.map((r) => (
              <span key={r.id} className={`badge ${r.enabled ? 'badge-info' : 'badge-neutral'}`} style={{ gap: 6 }}>
                <span
                  style={{ cursor: 'pointer' }}
                  onClick={async () => {
                    await api.updateSseRule(r.id, { ...r, enabled: !r.enabled })
                    props.onRefreshSseRules()
                  }}
                >
                  [{r.enabled ? 'ON' : 'OFF'}] SSE {r.event_type_pattern}
                </span>
                <Trash2
                  size={10}
                  style={{ cursor: 'pointer' }}
                  onClick={async () => {
                    await api.deleteSseRule(r.id)
                    props.onRefreshSseRules()
                  }}
                />
              </span>
            ))}
          </div>
        )}
      </div>

      {props.breakpoints.length === 0 ? (
        <div className="empty-state">
          No traffic is currently suspended. Use the <strong>Quick Breakpoint Watcher</strong> bar above to dynamically intercept live HTTP requests, real upstream HTTP responses, or SSE event frames.
        </div>
      ) : (
        <div className="split-pane">
          <div className="pane-left">
            <table className="data-table">
              <thead>
                <tr>
                  <th>Stage</th>
                  <th>Target</th>
                  <th>Rule</th>
                  <th>Directory</th>
                </tr>
              </thead>
              <tbody>
                {props.breakpoints.map((bp) => (
                  <tr
                    key={bp.id}
                    className={selected?.id === bp.id ? 'selected' : ''}
                    onClick={() => setSelectedId(bp.id)}
                  >
                    <td>
                      {bp.kind === 'http_response' ? (
                        <span className="badge badge-warn">HTTP RES ({bp.res_status ?? 200})</span>
                      ) : bp.kind === 'sse_event' ? (
                        <span className="badge badge-info">SSE FRAME</span>
                      ) : (
                        <span className="badge badge-ok">HTTP REQ</span>
                      )}
                    </td>
                    <td className="mono">
                      <span className={`method-${bp.method}`} style={{ fontWeight: 600, marginRight: 6 }}>
                        {bp.method}
                      </span>
                      {bp.path}
                    </td>
                    <td>
                      <span className="badge badge-neutral">{bp.rule_name}</span>
                    </td>
                    <td className="mono" style={{ color: 'var(--text-muted)' }}>
                      {bp.directory ?? '—'}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>

          {selected && (
            <aside className="pane-right" style={{ width: '58%', maxWidth: 820 }}>
              {selected.kind === 'http_request' && (
                <>
                  <div className="inspector-header">
                    <span className="mono" style={{ fontWeight: 600 }}>
                      HTTP Request Suspended: {selected.method} {selected.path}
                    </span>
                    <div className="toolbar-group">
                      <button className="btn btn-sm btn-primary" onClick={forwardRequest}>
                        <Play size={12} /> Forward to Upstream
                      </button>
                      <button className="btn btn-sm btn-warn" onClick={() => respondHttp(false)}>
                        <Send size={12} /> Direct Respond
                      </button>
                      <button className="btn btn-sm btn-danger" onClick={abortHttp}>
                        <OctagonX size={12} /> Abort
                      </button>
                    </div>
                  </div>

                  <div className="inspector-body">
                    <div className="form-grid">
                      <div className="field">
                        <label>Method</label>
                        <input
                          className="input mono"
                          value={editMethod}
                          onChange={(e) => setEditMethod(e.target.value)}
                        />
                      </div>
                      <div className="field">
                        <label>Path</label>
                        <input
                          className="input mono"
                          value={editPath}
                          onChange={(e) => setEditPath(e.target.value)}
                        />
                      </div>
                      <div className="field">
                        <label>Query</label>
                        <input
                          className="input mono"
                          value={editQuery}
                          onChange={(e) => setEditQuery(e.target.value)}
                        />
                      </div>
                    </div>

                    <div className="field">
                      <label>Editable Request Headers (JSON)</label>
                      <textarea
                        className="textarea"
                        rows={3}
                        value={reqHeadersText}
                        onChange={(e) => setReqHeadersText(e.target.value)}
                      />
                    </div>

                    <div className="field">
                      <label>Editable Request Body (sent upstream when clicking Forward)</label>
                      <textarea
                        className="textarea"
                        rows={6}
                        value={reqBody}
                        onChange={(e) => setReqBody(e.target.value)}
                      />
                    </div>

                    <div className="panel" style={{ padding: 10 }}>
                      <div className="panel-title" style={{ fontSize: 12 }}>
                        <span>Or Short-Circuit with Direct Mock Response</span>
                      </div>
                      <div className="form-grid" style={{ marginBottom: 8 }}>
                        <div className="field">
                          <label>Mock Status</label>
                          <input
                            className="input mono"
                            type="number"
                            value={resStatus}
                            onChange={(e) => setResStatus(Number(e.target.value))}
                          />
                        </div>
                      </div>
                      <textarea
                        className="textarea"
                        rows={5}
                        value={resBody}
                        onChange={(e) => setResBody(e.target.value)}
                      />
                    </div>
                  </div>
                </>
              )}

              {selected.kind === 'http_response' && (
                <>
                  <div className="inspector-header">
                    <span className="mono" style={{ fontWeight: 600 }}>
                      Upstream Response Suspended: {selected.method} {selected.path}
                    </span>
                    <div className="toolbar-group">
                      <button className="btn btn-sm btn-primary" onClick={() => respondHttp(false)}>
                        <Play size={12} /> Resume Response
                      </button>
                      <button className="btn btn-sm btn-warn" onClick={() => respondHttp(true)}>
                        <Save size={12} /> Save Rule &amp; Resume
                      </button>
                      <button className="btn btn-sm btn-danger" onClick={abortHttp}>
                        <OctagonX size={12} /> Abort
                      </button>
                    </div>
                  </div>

                  <div className="inspector-body">
                    <div className="form-grid">
                      <div className="field">
                        <label>Editable HTTP Response Status</label>
                        <input
                          className="input mono"
                          type="number"
                          value={resStatus}
                          onChange={(e) => setResStatus(Number(e.target.value))}
                        />
                      </div>
                    </div>

                    <div className="field">
                      <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
                        <label>Real Upstream Response Body (edit freely before returning to client)</label>
                        <button
                          className="btn btn-sm"
                          onClick={() => {
                            try {
                              setResBody(JSON.stringify(JSON.parse(resBody), null, 2))
                            } catch {}
                          }}
                        >
                          Format JSON
                        </button>
                      </div>
                      <textarea
                        className="textarea"
                        rows={13}
                        value={resBody}
                        onChange={(e) => setResBody(e.target.value)}
                      />
                    </div>

                    <div className="field">
                      <label>Editable Response Headers (JSON)</label>
                      <textarea
                        className="textarea"
                        rows={3}
                        value={resHeadersText}
                        onChange={(e) => setResHeadersText(e.target.value)}
                      />
                    </div>
                  </div>
                </>
              )}

              {selected.kind === 'sse_event' && (
                <>
                  <div className="inspector-header">
                    <span className="mono" style={{ fontWeight: 600 }}>
                      SSE Frame Suspended: {selected.sse_event_type ?? selected.path}
                    </span>
                    <div className="toolbar-group">
                      <button className="btn btn-sm btn-primary" onClick={() => emitEditedSse(false)}>
                        <Play size={12} /> Forward Edited Event
                      </button>
                      <button className="btn btn-sm btn-warn" onClick={() => emitEditedSse(true)}>
                        <Save size={12} /> Save Rule &amp; Forward
                      </button>
                      <button className="btn btn-sm btn-danger" onClick={dropSseFrame}>
                        <OctagonX size={12} /> Drop Frame
                      </button>
                    </div>
                  </div>

                  <div className="inspector-body">
                    <div className="toolbar-group">
                      <div className="field" style={{ flex: 1 }}>
                        <label>Target Directory</label>
                        <input
                          className="input mono"
                          value={sseDir}
                          onChange={(e) => setSseDir(e.target.value)}
                        />
                      </div>
                      <label
                        style={{
                          display: 'inline-flex',
                          alignItems: 'center',
                          gap: 6,
                          fontSize: 12,
                          paddingTop: 16,
                        }}
                      >
                        <input
                          type="checkbox"
                          checked={sseShadow}
                          onChange={(e) => setSseShadow(e.target.checked)}
                        />
                        Apply to ShadowStore
                      </label>
                    </div>

                    <div className="field">
                      <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
                        <label>Live SSE Event Payload JSON (edit freely or click Drop Frame)</label>
                        <button
                          className="btn btn-sm"
                          onClick={() => {
                            try {
                              setSsePayloadText(JSON.stringify(JSON.parse(ssePayloadText), null, 2))
                            } catch {}
                          }}
                        >
                          Format JSON
                        </button>
                      </div>
                      <textarea
                        className="textarea"
                        rows={15}
                        value={ssePayloadText}
                        onChange={(e) => setSsePayloadText(e.target.value)}
                      />
                    </div>
                  </div>
                </>
              )}
            </aside>
          )}
        </div>
      )}
    </>
  )
}
