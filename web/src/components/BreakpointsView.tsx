import { useEffect, useState } from 'react'
import { OctagonX, Play, Send } from 'lucide-react'
import { api } from '../api'
import type { PendingBreakpoint } from '../types'

export function BreakpointsView(props: {
  breakpoints: PendingBreakpoint[]
  onRefresh: () => void
  onNotify: (msg: string, isErr?: boolean) => void
}) {
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const selected = props.breakpoints.find((b) => b.id === selectedId) ?? props.breakpoints[0] ?? null

  const [reqBody, setReqBody] = useState('')
  const [resStatus, setResStatus] = useState(200)
  const [resBody, setResBody] = useState('{\n  "ok": true\n}')

  useEffect(() => {
    if (selected) {
      setReqBody(selected.req_body ?? '')
    }
  }, [selected?.id])

  const forward = async () => {
    if (!selected) return
    try {
      await api.resolveBreakpoint(selected.id, {
        decision: 'forward',
        req_body: reqBody || null,
        req_headers: null,
      })
      props.onRefresh()
      props.onNotify(`Forwarded ${selected.method} ${selected.path} to upstream`)
    } catch (e) {
      props.onNotify(String(e), true)
    }
  }

  const respond = async () => {
    if (!selected) return
    try {
      await api.resolveBreakpoint(selected.id, {
        decision: 'respond',
        status: resStatus,
        headers: { 'content-type': 'application/json' },
        body: resBody,
      })
      props.onRefresh()
      props.onNotify(`Responded directly with HTTP ${resStatus}`)
    } catch (e) {
      props.onNotify(String(e), true)
    }
  }

  const abort = async () => {
    if (!selected) return
    try {
      await api.resolveBreakpoint(selected.id, {
        decision: 'abort',
        message: 'Aborted from oc-mock-server DevTools',
      })
      props.onRefresh()
      props.onNotify('Aborted request with HTTP 502')
    } catch (e) {
      props.onNotify(String(e), true)
    }
  }

  if (props.breakpoints.length === 0) {
    return (
      <div className="empty-state">
        No requests are currently suspended. Add a <strong>Suspend Breakpoint</strong> rule under Mock &amp; AI to intercept live requests before forwarding.
      </div>
    )
  }

  return (
    <div className="split-pane">
      <div className="pane-left">
        <table className="data-table">
          <thead>
            <tr>
              <th>Rule</th>
              <th>Method</th>
              <th>Path</th>
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
                  <span className="badge badge-warn">{bp.rule_name}</span>
                </td>
                <td className={`mono method-${bp.method}`} style={{ fontWeight: 600 }}>
                  {bp.method}
                </td>
                <td className="mono">{bp.path}</td>
                <td className="mono" style={{ color: 'var(--text-muted)' }}>
                  {bp.directory ?? '—'}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      {selected && (
        <aside className="pane-right" style={{ width: '55%' }}>
          <div className="inspector-header">
            <span className="mono" style={{ fontWeight: 600 }}>
              Suspended: {selected.method} {selected.path}
            </span>
            <div className="toolbar-group">
              <button className="btn btn-sm btn-primary" onClick={forward}>
                <Play size={12} /> Forward to Upstream
              </button>
              <button className="btn btn-sm btn-warn" onClick={respond}>
                <Send size={12} /> Direct Respond
              </button>
              <button className="btn btn-sm btn-danger" onClick={abort}>
                <OctagonX size={12} /> Abort
              </button>
            </div>
          </div>

          <div className="inspector-body">
            <div className="field">
              <label>Editable Request Body (used when clicking Forward)</label>
              <textarea
                className="textarea"
                rows={6}
                value={reqBody}
                onChange={(e) => setReqBody(e.target.value)}
              />
            </div>

            <div className="form-grid">
              <div className="field">
                <label>Direct Mock Response Status</label>
                <input
                  className="input mono"
                  type="number"
                  value={resStatus}
                  onChange={(e) => setResStatus(Number(e.target.value))}
                />
              </div>
            </div>

            <div className="field">
              <label>Direct Mock Response Body (used when clicking Direct Respond)</label>
              <textarea
                className="textarea"
                rows={8}
                value={resBody}
                onChange={(e) => setResBody(e.target.value)}
              />
            </div>
          </div>
        </aside>
      )}
    </div>
  )
}
