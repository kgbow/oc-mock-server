import { useEffect, useState } from 'react'
import {
  Download,
  ExternalLink,
  Pause,
  Play,
  SkipForward,
  Square,
  Terminal,
  Trash2,
  Upload,
} from 'lucide-react'
import { api } from '../api'
import type { ReplayStatus, SessionOverview, TapeSummary } from '../types'

export function ReplayView(props: {
  tapes: TapeSummary[]
  sessions: SessionOverview[]
  replay: ReplayStatus
  activeDirectories: string[]
  onRefreshTapes: () => void
  onNotify: (msg: string, isErr?: boolean) => void
}) {
  const [targetDir, setTargetDir] = useState(props.activeDirectories[0] ?? '/workspace/project')
  const [mountShadow, setMountShadow] = useState(true)
  const [speed, setSpeed] = useState(props.replay.speed || 1)
  const [extractSid, setExtractSid] = useState('')
  const [importText, setImportText] = useState('')
  const [showImport, setShowImport] = useState(false)
  const [shadowSnapshot, setShadowSnapshot] = useState<unknown>(null)

  useEffect(() => {
    if (!targetDir && props.activeDirectories[0]) {
      setTargetDir(props.activeDirectories[0])
    }
  }, [props.activeDirectories])

  useEffect(() => {
    if (!props.replay.session_id) {
      setShadowSnapshot(null)
      return
    }
    api
      .getSessionState(props.replay.session_id)
      .then(setShadowSnapshot)
      .catch(() => {})
  }, [props.replay.session_id, props.replay.cursor])

  const loadTape = async (tapeId: string) => {
    try {
      await api.replayLoad({
        tape_id: tapeId,
        target_directory: targetDir || props.activeDirectories[0],
        mount_shadow: mountShadow,
        speed,
        min_frame_ms: 20,
      })
      props.onNotify(`Loaded tape ${tapeId} into ShadowStore (ready to play)`)
    } catch (e) {
      props.onNotify(String(e), true)
    }
  }

  const handleExtract = async () => {
    if (!extractSid) return
    try {
      const t = await api.extractTape(extractSid)
      props.onRefreshTapes()
      props.onNotify(`Extracted tape "${t.name}" (${t.event_count} events)`)
    } catch (e) {
      props.onNotify(String(e), true)
    }
  }

  const handleImport = async () => {
    if (!importText.trim()) return
    try {
      const t = await api.importTape(importText)
      setShowImport(false)
      setImportText('')
      props.onRefreshTapes()
      props.onNotify(`Imported tape "${t.name}" (${t.event_count} events)`)
    } catch (e) {
      props.onNotify(String(e), true)
    }
  }

  const handleExport = async (tape: TapeSummary) => {
    try {
      const jsonl = await api.exportTape(tape.id)
      const blob = new Blob([jsonl], { type: 'application/x-ndjson' })
      const url = URL.createObjectURL(blob)
      const a = document.createElement('a')
      a.href = url
      a.download = `${tape.id}.tape.jsonl`
      a.click()
      URL.revokeObjectURL(url)
    } catch (e) {
      props.onNotify(String(e), true)
    }
  }

  const handleSpeedChange = async (next: number) => {
    setSpeed(next)
    try {
      await api.replaySpeed(next)
    } catch {}
  }

  const handleTuiSelect = async () => {
    try {
      await api.replayTuiSelect(props.replay.session_id ?? undefined)
      props.onNotify('Sent tui.session.select event to attached TUI clients')
    } catch (e) {
      props.onNotify(String(e), true)
    }
  }

  return (
    <>
      <div className="replay-deck">
        <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', flexWrap: 'wrap', gap: 10 }}>
          <div className="toolbar-group">
            <button
              className="btn btn-primary"
              disabled={!props.replay.tape_id}
              onClick={() =>
                (props.replay.state === 'playing' ? api.replayPause() : api.replayPlay()).catch((e) =>
                  props.onNotify(String(e), true),
                )
              }
            >
              {props.replay.state === 'playing' ? <Pause size={13} /> : <Play size={13} />}
              {props.replay.state === 'playing' ? 'Pause' : 'Play'}
            </button>
            <button
              className="btn"
              disabled={!props.replay.tape_id}
              onClick={() => api.replayStep().catch((e) => props.onNotify(String(e), true))}
            >
              <SkipForward size={13} /> Step
            </button>
            <button
              className="btn"
              disabled={!props.replay.tape_id}
              onClick={() => api.replayStop().catch((e) => props.onNotify(String(e), true))}
            >
              <Square size={12} /> Stop
            </button>

            <select
              className="select mono"
              value={speed}
              onChange={(e) => handleSpeedChange(Number(e.target.value))}
            >
              <option value={0.5}>0.5x</option>
              <option value={1}>1.0x</option>
              <option value={2}>2.0x</option>
              <option value={5}>5.0x</option>
              <option value={10}>10.0x</option>
            </select>

            <span className={`badge ${props.replay.state === 'playing' ? 'badge-ok' : 'badge-neutral'}`}>
              {props.replay.state.toUpperCase()}
            </span>
          </div>

          <div className="toolbar-group">
            <button
              className="btn btn-sm"
              disabled={!props.replay.session_id}
              onClick={handleTuiSelect}
              title="Send tui.session.select to TUI clients"
            >
              <Terminal size={12} /> Select in TUI
            </button>
            {props.replay.web_session_url && (
              <a
                className="btn btn-sm"
                href={props.replay.web_session_url}
                target="_blank"
                rel="noreferrer"
                style={{ textDecoration: 'none' }}
              >
                <ExternalLink size={12} /> Open Client Session URL
              </a>
            )}
          </div>
        </div>

        <div className="scrubber-row">
          <span className="mono" style={{ fontSize: 11.5, color: 'var(--text-secondary)', minWidth: 160 }}>
            {props.replay.tape_name ?? 'No tape loaded'}
          </span>
          <input
            className="scrubber"
            type="range"
            min={0}
            max={Math.max(props.replay.total_events, 1)}
            value={props.replay.cursor}
            disabled={!props.replay.tape_id}
            onChange={(e) => api.replaySeek(Number(e.target.value), true).catch(() => {})}
          />
          <span className="mono" style={{ fontSize: 11.5, color: 'var(--text-muted)', minWidth: 74, textAlign: 'right' }}>
            {props.replay.cursor} / {props.replay.total_events}
          </span>
        </div>

        <div className="toolbar-group">
          <span style={{ fontSize: 11.5, color: 'var(--text-secondary)' }}>Directory Remap:</span>
          <input
            className="input mono"
            style={{ width: 260 }}
            value={targetDir}
            onChange={(e) => setTargetDir(e.target.value)}
            list="active-dirs"
          />
          <datalist id="active-dirs">
            {props.activeDirectories.map((d) => (
              <option key={d} value={d} />
            ))}
          </datalist>
          <label style={{ display: 'inline-flex', alignItems: 'center', gap: 5, fontSize: 12, color: 'var(--text-secondary)' }}>
            <input type="checkbox" checked={mountShadow} onChange={(e) => setMountShadow(e.target.checked)} />
            Mount ShadowStore into GET /session*
          </label>
        </div>
      </div>

      <div className="split-pane">
        <div className="pane-left" style={{ padding: 14, gap: 12 }}>
          <div className="toolbar-group" style={{ justifyContent: 'space-between' }}>
            <div className="toolbar-group">
              <select className="select mono" value={extractSid} onChange={(e) => setExtractSid(e.target.value)}>
                <option value="">Extract tape from captured session...</option>
                {props.sessions.map((s) => (
                  <option key={s.session_id} value={s.session_id}>
                    {s.session_id} ({s.sse_count} SSE)
                  </option>
                ))}
              </select>
              <button className="btn btn-sm" disabled={!extractSid} onClick={handleExtract}>
                Extract Tape
              </button>
            </div>
            <button className="btn btn-sm" onClick={() => setShowImport((v) => !v)}>
              <Upload size={12} /> Import .tape.jsonl
            </button>
          </div>

          {showImport && (
            <div className="panel">
              <div className="panel-title">Import Portable .tape.jsonl</div>
              <textarea
                className="textarea"
                rows={6}
                placeholder="Paste .tape.jsonl content here..."
                value={importText}
                onChange={(e) => setImportText(e.target.value)}
              />
              <div className="toolbar-group" style={{ marginTop: 8 }}>
                <button className="btn btn-sm btn-primary" onClick={handleImport}>
                  Import Tape
                </button>
              </div>
            </div>
          )}

          <div className="panel" style={{ padding: 0, overflow: 'hidden' }}>
            <table className="data-table">
              <thead>
                <tr>
                  <th>Tape Name</th>
                  <th>Session ID</th>
                  <th>Frames</th>
                  <th>Duration</th>
                  <th></th>
                </tr>
              </thead>
              <tbody>
                {props.tapes.map((t) => (
                  <tr key={t.id} className={props.replay.tape_id === t.id ? 'selected' : ''}>
                    <td style={{ fontWeight: 500 }}>
                      {t.name}
                      {t.description && (
                        <div style={{ fontSize: 11, color: 'var(--text-muted)' }}>{t.description}</div>
                      )}
                    </td>
                    <td className="mono">{t.session_id ?? '—'}</td>
                    <td className="mono">{t.event_count}</td>
                    <td className="mono">{t.duration_ms}ms</td>
                    <td style={{ textAlign: 'right' }}>
                      <div className="toolbar-group" style={{ justifyContent: 'flex-end' }}>
                        <button className="btn btn-sm btn-primary" onClick={() => loadTape(t.id)}>
                          Load
                        </button>
                        <button className="btn btn-sm" onClick={() => handleExport(t)} title="Export .tape.jsonl">
                          <Download size={12} />
                        </button>
                        <button
                          className="btn btn-sm btn-danger"
                          onClick={async () => {
                            await api.deleteTape(t.id)
                            props.onRefreshTapes()
                          }}
                        >
                          <Trash2 size={12} />
                        </button>
                      </div>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>

        <aside className="pane-right">
          <div className="inspector-header">
            <span className="mono" style={{ fontWeight: 600 }}>
              Live ShadowStore Projection
            </span>
            <span className="badge badge-info">{props.replay.session_id ?? 'idle'}</span>
          </div>
          <div className="inspector-body">
            {shadowSnapshot ? (
              <pre className="code-block">{JSON.stringify(shadowSnapshot, null, 2)}</pre>
            ) : (
              <div className="empty-state">
                Load and play/step a tape to inspect the live ShadowStore state served to OpenCode clients on <code>GET /session*</code>.
              </div>
            )}
          </div>
        </aside>
      </div>
    </>
  )
}
