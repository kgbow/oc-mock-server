import { useState } from 'react'
import { AlertTriangle, Eye, RefreshCw } from 'lucide-react'
import { api } from '../api'
import type { Anomaly, SessionOverview } from '../types'

export function AnomaliesView(props: {
  anomalies: Anomaly[]
  sessions: SessionOverview[]
  onRefresh: () => void
  onNotify: (msg: string, isErr?: boolean) => void
}) {
  const [selected, setSelected] = useState<Anomaly | null>(null)
  const [stateSession, setStateSession] = useState('')
  const [stateSeq, setStateSeq] = useState('')
  const [stateSnapshot, setStateSnapshot] = useState<unknown>(null)

  const inspectState = async (sid: string, seqStr?: string) => {
    setStateSession(sid)
    if (seqStr !== undefined) setStateSeq(seqStr)
    const seqNum = seqStr && seqStr.trim() !== '' ? Number(seqStr) : undefined
    try {
      const snap = await api.getSessionState(sid, Number.isFinite(seqNum) ? seqNum : undefined)
      setStateSnapshot(snap)
    } catch (e) {
      props.onNotify(String(e), true)
    }
  }

  return (
    <>
      <div className="toolbar">
        <div className="toolbar-group">
          <button className="btn btn-sm" onClick={props.onRefresh}>
            <RefreshCw size={12} /> Rescan Traffic
          </button>
          <span className="mono" style={{ color: 'var(--text-muted)', fontSize: 11.5 }}>
            {props.anomalies.length} anomalies detected
          </span>
        </div>

        <div className="toolbar-group">
          <select
            className="select mono"
            value={stateSession}
            onChange={(e) => setStateSession(e.target.value)}
          >
            <option value="">Reconstruct ShadowState for session...</option>
            {props.sessions.map((s) => (
              <option key={s.session_id} value={s.session_id}>
                {s.session_id}
              </option>
            ))}
          </select>
          <input
            className="input mono"
            style={{ width: 110 }}
            placeholder="at_seq (opt)"
            value={stateSeq}
            onChange={(e) => setStateSeq(e.target.value)}
          />
          <button
            className="btn btn-sm btn-primary"
            disabled={!stateSession}
            onClick={() => inspectState(stateSession, stateSeq)}
          >
            <Eye size={12} /> Inspect State
          </button>
        </div>
      </div>

      <div className="split-pane">
        <div className="pane-left">
          {props.anomalies.length === 0 ? (
            <div className="empty-state">
              <AlertTriangle size={18} style={{ marginBottom: 6, opacity: 0.5 }} />
              <div>No HTTP errors, slow requests, orphan deltas, reconnect storms, or stuck prompts detected.</div>
            </div>
          ) : (
            <table className="data-table">
              <thead>
                <tr>
                  <th>Severity</th>
                  <th>Kind</th>
                  <th>Session</th>
                  <th>Ref</th>
                  <th>Message</th>
                </tr>
              </thead>
              <tbody>
                {props.anomalies.map((a, idx) => (
                  <tr
                    key={`${a.ref_id}-${a.kind}-${idx}`}
                    className={selected === a ? 'selected' : ''}
                    onClick={() => setSelected(a)}
                  >
                    <td>
                      <span
                        className={`badge ${
                          a.severity === 'error'
                            ? 'badge-err'
                            : a.severity === 'warning'
                              ? 'badge-warn'
                              : 'badge-info'
                        }`}
                      >
                        {a.severity.toUpperCase()}
                      </span>
                    </td>
                    <td className="mono">{a.kind}</td>
                    <td className="mono">{a.session_id ?? '—'}</td>
                    <td className="mono" style={{ color: 'var(--text-muted)' }}>
                      {a.ref_id}
                    </td>
                    <td>{a.message}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </div>

        <aside className="pane-right">
          <div className="inspector-header">
            <span className="mono" style={{ fontWeight: 600 }}>
              {stateSnapshot ? `ShadowState Snapshot (${stateSession})` : selected ? selected.kind : 'Diagnostics Inspector'}
            </span>
            {selected?.session_id && (
              <button
                className="btn btn-sm btn-primary"
                onClick={() => {
                  const seqMatch = selected.ref_id.startsWith('sse:') ? selected.ref_id.slice(4) : ''
                  inspectState(selected.session_id!, seqMatch)
                }}
              >
                Reconstruct State Here
              </button>
            )}
          </div>
          <div className="inspector-body">
            {stateSnapshot ? (
              <pre className="code-block">{JSON.stringify(stateSnapshot, null, 2)}</pre>
            ) : selected ? (
              <>
                <div style={{ fontSize: 12.5 }}>{selected.message}</div>
                <pre className="code-block">{JSON.stringify(selected.details, null, 2)}</pre>
              </>
            ) : (
              <div className="empty-state">
                Select an anomaly or pick a session above to inspect its reconstructed ShadowStore state at any SSE sequence number.
              </div>
            )}
          </div>
        </aside>
      </div>
    </>
  )
}
