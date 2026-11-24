import { useEffect, useState } from 'react'
import {
  ArrowDown,
  ArrowUp,
  Copy,
  Download,
  Edit3,
  ExternalLink,
  Pause,
  Play,
  Plus,
  Save,
  SkipForward,
  Square,
  Terminal,
  Trash2,
  Upload,
  X,
} from 'lucide-react'
import { api } from '../api'
import type {
  OpenApiSummary,
  ReplayStatus,
  SessionOverview,
  Tape,
  TapeEvent,
  TapeSummary,
} from '../types'

interface EditableFrame {
  offset_ms: number
  directory: string
  payloadText: string
}

export function ReplayView(props: {
  tapes: TapeSummary[]
  sessions: SessionOverview[]
  openapi: OpenApiSummary | null
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

  // Visual Tape Editor state
  const [editingTape, setEditingTape] = useState<Tape | null>(null)
  const [tapeName, setTapeName] = useState('')
  const [tapeDesc, setTapeDesc] = useState('')
  const [tapeDir, setTapeDir] = useState('')
  const [tapeSid, setTapeSid] = useState('')
  const [tapeSnapText, setTapeSnapText] = useState('{}')
  const [frames, setFrames] = useState<EditableFrame[]>([])
  const [selFrameIdx, setSelFrameIdx] = useState(0)

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

  const openTapeEditor = async (tapeId?: string) => {
    if (!tapeId) {
      const defDir = props.activeDirectories[0] ?? '/workspace/project'
      const defSid = `ses_custom_${Math.random().toString(36).slice(2, 8)}`
      setEditingTape({
        id: '',
        name: 'Custom SSE Scenario Tape',
        description: 'Manually authored SSE sequence',
        directory: defDir,
        session_id: defSid,
        snapshot: {},
        events: [],
        created_at: 0,
      })
      setTapeName('Custom SSE Scenario Tape')
      setTapeDesc('Manually authored SSE sequence')
      setTapeDir(defDir)
      setTapeSid(defSid)
      setTapeSnapText('{}')
      setFrames([
        {
          offset_ms: 0,
          directory: defDir,
          payloadText: JSON.stringify(
            {
              type: 'session.created',
              properties: {
                info: {
                  id: defSid,
                  directory: defDir,
                  title: 'Custom Mocked Session',
                  time: { created: Date.now(), updated: Date.now() },
                },
              },
            },
            null,
            2,
          ),
        },
      ])
      setSelFrameIdx(0)
      return
    }

    try {
      const t = await api.getTape(tapeId)
      setEditingTape(t)
      setTapeName(t.name)
      setTapeDesc(t.description ?? '')
      setTapeDir(t.directory)
      setTapeSid(t.session_id ?? '')
      setTapeSnapText(JSON.stringify(t.snapshot ?? {}, null, 2))
      setFrames(
        t.events.map((ev) => ({
          offset_ms: ev.offset_ms,
          directory: ev.directory,
          payloadText: JSON.stringify(ev.payload, null, 2),
        })),
      )
      setSelFrameIdx(0)
    } catch (e) {
      props.onNotify(String(e), true)
    }
  }

  const saveEditedTape = async () => {
    if (!editingTape) return
    let parsedSnap: Record<string, unknown> = {}
    try {
      parsedSnap = JSON.parse(tapeSnapText || '{}') as Record<string, unknown>
    } catch {
      props.onNotify('Initial Snapshot JSON is invalid', true)
      return
    }

    const events: TapeEvent[] = []
    for (let i = 0; i < frames.length; i++) {
      try {
        events.push({
          offset_ms: Math.max(0, Number(frames[i].offset_ms) || 0),
          directory: frames[i].directory.trim() || tapeDir || '/workspace/project',
          payload: JSON.parse(frames[i].payloadText),
        })
      } catch {
        props.onNotify(`Frame #${i + 1} payload is not valid JSON`, true)
        return
      }
    }

    try {
      const saved = await api.saveTape({
        id: editingTape.id || undefined,
        name: tapeName.trim() || 'Untitled Tape',
        description: tapeDesc.trim() || null,
        directory: tapeDir.trim() || '/workspace/project',
        session_id: tapeSid.trim() || null,
        snapshot: parsedSnap,
        events,
        created_at: editingTape.created_at,
      })
      props.onRefreshTapes()
      setEditingTape(saved)
      props.onNotify(`Saved tape "${saved.name}" (${saved.events.length} SSE frames)`)
    } catch (e) {
      props.onNotify(String(e), true)
    }
  }

  const addEditorFrame = async (evType = 'message.part.delta') => {
    const lastOffset = frames.length > 0 ? frames[frames.length - 1].offset_ms + 200 : 0
    let sample: unknown = {
      type: evType,
      properties: { sessionID: tapeSid || 'ses_custom_001' },
    }
    try {
      sample = await api.fakeSse(evType, tapeDir || props.activeDirectories[0])
    } catch {}
    const nextIdx = frames.length
    setFrames((prev) => [
      ...prev,
      {
        offset_ms: lastOffset,
        directory: tapeDir || props.activeDirectories[0] || '/workspace/project',
        payloadText: JSON.stringify(sample, null, 2),
      },
    ])
    setSelFrameIdx(nextIdx)
  }

  const moveFrame = (idx: number, delta: -1 | 1) => {
    const target = idx + delta
    if (target < 0 || target >= frames.length) return
    setFrames((prev) => {
      const copy = [...prev]
      const tmp = copy[idx]
      copy[idx] = copy[target]
      copy[target] = tmp
      return copy
    })
    setSelFrameIdx(target)
  }

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

  const currentFrame = frames[selFrameIdx] ?? null

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
            <button className="btn btn-sm" onClick={() => openTapeEditor()}>
              <Plus size={12} /> New Custom Tape
            </button>
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

      {editingTape ? (
        <div className="split-pane">
          <div className="pane-left" style={{ padding: 14, gap: 12 }}>
            <div className="panel">
              <div className="panel-title">
                <span>Tape Metadata — {editingTape.id || 'New Tape'}</span>
                <div className="toolbar-group">
                  <button className="btn btn-sm btn-primary" onClick={saveEditedTape}>
                    <Save size={12} /> Save Tape
                  </button>
                  {editingTape.id && (
                    <button className="btn btn-sm" onClick={() => loadTape(editingTape.id)}>
                      <Play size={12} /> Load in Player
                    </button>
                  )}
                  <button className="btn btn-sm btn-ghost" onClick={() => setEditingTape(null)}>
                    <X size={14} /> Close Editor
                  </button>
                </div>
              </div>
              <div className="form-grid">
                <div className="field">
                  <label>Tape Name</label>
                  <input className="input" value={tapeName} onChange={(e) => setTapeName(e.target.value)} />
                </div>
                <div className="field">
                  <label>Session ID</label>
                  <input className="input mono" value={tapeSid} onChange={(e) => setTapeSid(e.target.value)} />
                </div>
                <div className="field">
                  <label>Default Directory</label>
                  <input className="input mono" value={tapeDir} onChange={(e) => setTapeDir(e.target.value)} />
                </div>
                <div className="field">
                  <label>Description</label>
                  <input className="input" value={tapeDesc} onChange={(e) => setTapeDesc(e.target.value)} />
                </div>
              </div>
            </div>

            <div className="toolbar-group" style={{ justifyContent: 'space-between' }}>
              <span style={{ fontWeight: 600, fontSize: 12.5 }}>SSE Event Frames ({frames.length})</span>
              <div className="toolbar-group">
                <select
                  className="select"
                  defaultValue=""
                  onChange={(e) => {
                    if (e.target.value) {
                      addEditorFrame(e.target.value)
                      e.target.value = ''
                    }
                  }}
                >
                  <option value="">+ Add Frame from OpenAPI...</option>
                  {(props.openapi?.events ?? []).map((ev) => (
                    <option key={ev.event_type} value={ev.event_type}>
                      {ev.event_type}
                    </option>
                  ))}
                </select>
                <button className="btn btn-sm" onClick={() => addEditorFrame('message.part.delta')}>
                  <Plus size={12} /> Add Frame
                </button>
              </div>
            </div>

            <div className="panel" style={{ padding: 0, overflow: 'hidden' }}>
              <table className="data-table">
                <thead>
                  <tr>
                    <th>#</th>
                    <th>Offset (ms)</th>
                    <th>Event Type</th>
                    <th>Preview</th>
                    <th></th>
                  </tr>
                </thead>
                <tbody>
                  {frames.map((f, idx) => {
                    let evType = 'invalid-json'
                    try {
                      evType = String((JSON.parse(f.payloadText) as { type?: string })?.type ?? 'unknown')
                    } catch {}
                    return (
                      <tr
                        key={idx}
                        className={selFrameIdx === idx ? 'selected' : ''}
                        onClick={() => setSelFrameIdx(idx)}
                      >
                        <td className="mono" style={{ color: 'var(--text-muted)' }}>
                          {idx + 1}
                        </td>
                        <td className="mono">+{f.offset_ms}ms</td>
                        <td className="mono" style={{ color: 'var(--accent-strong)', fontWeight: 500 }}>
                          {evType}
                        </td>
                        <td className="mono" style={{ color: 'var(--text-secondary)' }}>
                          {f.payloadText.replace(/\s+/g, ' ').slice(0, 90)}
                        </td>
                        <td style={{ textAlign: 'right' }} onClick={(e) => e.stopPropagation()}>
                          <div className="toolbar-group" style={{ justifyContent: 'flex-end', gap: 3 }}>
                            <button className="btn btn-sm" onClick={() => moveFrame(idx, -1)} title="Move up">
                              <ArrowUp size={11} />
                            </button>
                            <button className="btn btn-sm" onClick={() => moveFrame(idx, 1)} title="Move down">
                              <ArrowDown size={11} />
                            </button>
                            <button
                              className="btn btn-sm"
                              onClick={() => {
                                setFrames((prev) => [
                                  ...prev.slice(0, idx + 1),
                                  { ...prev[idx], offset_ms: prev[idx].offset_ms + 100 },
                                  ...prev.slice(idx + 1),
                                ])
                                setSelFrameIdx(idx + 1)
                              }}
                              title="Duplicate frame"
                            >
                              <Copy size={11} />
                            </button>
                            <button
                              className="btn btn-sm btn-danger"
                              onClick={() => {
                                setFrames((prev) => prev.filter((_, i) => i !== idx))
                                if (selFrameIdx >= frames.length - 1) {
                                  setSelFrameIdx(Math.max(0, frames.length - 2))
                                }
                              }}
                              title="Delete frame"
                            >
                              <Trash2 size={11} />
                            </button>
                          </div>
                        </td>
                      </tr>
                    )
                  })}
                </tbody>
              </table>
            </div>
          </div>

          <aside className="pane-right" style={{ width: '50%', maxWidth: 740 }}>
            <div className="inspector-header">
              <span className="mono" style={{ fontWeight: 600 }}>
                {currentFrame ? `Edit Frame #${selFrameIdx + 1}` : 'Initial Tape Snapshot'}
              </span>
              {currentFrame && (
                <button
                  className="btn btn-sm"
                  onClick={() => {
                    try {
                      const formatted = JSON.stringify(JSON.parse(currentFrame.payloadText), null, 2)
                      setFrames((prev) =>
                        prev.map((f, i) => (i === selFrameIdx ? { ...f, payloadText: formatted } : f)),
                      )
                    } catch {
                      props.onNotify('Not valid JSON', true)
                    }
                  }}
                >
                  Format JSON
                </button>
              )}
            </div>
            <div className="inspector-body">
              {currentFrame && (
                <>
                  <div className="form-grid">
                    <div className="field">
                      <label>Offset from Start (ms)</label>
                      <input
                        className="input mono"
                        type="number"
                        value={currentFrame.offset_ms}
                        onChange={(e) => {
                          const v = Number(e.target.value)
                          setFrames((prev) =>
                            prev.map((f, i) => (i === selFrameIdx ? { ...f, offset_ms: v } : f)),
                          )
                        }}
                      />
                    </div>
                    <div className="field">
                      <label>Frame Directory</label>
                      <input
                        className="input mono"
                        value={currentFrame.directory}
                        onChange={(e) => {
                          const v = e.target.value
                          setFrames((prev) =>
                            prev.map((f, i) => (i === selFrameIdx ? { ...f, directory: v } : f)),
                          )
                        }}
                      />
                    </div>
                  </div>
                  <div className="field">
                    <label>SSE Event Payload JSON</label>
                    <textarea
                      className="textarea"
                      rows={15}
                      value={currentFrame.payloadText}
                      onChange={(e) => {
                        const val = e.target.value
                        setFrames((prev) =>
                          prev.map((f, i) => (i === selFrameIdx ? { ...f, payloadText: val } : f)),
                        )
                      }}
                    />
                  </div>
                </>
              )}

              <div className="field">
                <label>Bootstrap Snapshot JSON (optional initial sessions/messages/todos)</label>
                <textarea
                  className="textarea"
                  rows={5}
                  value={tapeSnapText}
                  onChange={(e) => setTapeSnapText(e.target.value)}
                />
              </div>
            </div>
          </aside>
        </div>
      ) : (
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
                          <button
                            className="btn btn-sm"
                            onClick={() => openTapeEditor(t.id)}
                            title="Manually edit SSE frames & timestamps"
                          >
                            <Edit3 size={12} /> Edit
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
                  Load and play/step a tape to inspect the live ShadowStore state served to OpenCode clients on <code>GET /session*</code>, or click <strong>Edit</strong> on any tape to manually modify SSE frames.
                </div>
              )}
            </div>
          </aside>
        </div>
      )}
    </>
  )
}
