import { useEffect, useMemo, useState } from 'react'
import { Layers, RotateCcw, Save, Send, Sparkles, Wand2, X } from 'lucide-react'
import { api } from '../api'
import type { OpenApiSummary, SseMockActionType, SseMockRule, SseRecord } from '../types'

interface FoldedGroup {
  kind: 'folded'
  from_seq: number
  to_seq: number
  count: number
  session_id: string | null
  part_id: string
  preview: string
  directory: string
  source: SseRecord['source']
  records: SseRecord[]
}

type DisplayRow = { kind: 'single'; record: SseRecord } | FoldedGroup

function foldClientSide(records: SseRecord[]): DisplayRow[] {
  const chrono = [...records].reverse()
  const out: DisplayRow[] = []
  let acc: FoldedGroup | null = null

  const flush = () => {
    if (acc) {
      out.push(acc)
      acc = null
    }
  }

  for (const r of chrono) {
    if (r.event_type === 'message.part.delta' && !r.dropped) {
      const props = (r.payload as { properties?: { partID?: string; delta?: string } })?.properties
      const pid = props?.partID ?? r.part_id ?? ''
      const delta = props?.delta ?? ''
      if (acc && acc.part_id === pid && acc.session_id === r.session_id) {
        acc.to_seq = r.seq
        acc.count += 1
        acc.preview = (acc.preview + delta).slice(0, 140)
        acc.records.push(r)
      } else {
        flush()
        acc = {
          kind: 'folded',
          from_seq: r.seq,
          to_seq: r.seq,
          count: 1,
          session_id: r.session_id,
          part_id: pid,
          preview: delta.slice(0, 140),
          directory: r.directory,
          source: r.source,
          records: [r],
        }
      }
    } else {
      flush()
      out.push({ kind: 'single', record: r })
    }
  }
  flush()
  return out.reverse()
}

export function SseView(props: {
  events: SseRecord[]
  sseRules: SseMockRule[]
  openapi: OpenApiSummary | null
  activeDirectories: string[]
  onSseRulesChanged: () => void
  onNotify: (msg: string, isErr?: boolean) => void
}) {
  const [filterText, setFilterText] = useState('')
  const [foldDeltas, setFoldDeltas] = useState(true)
  const [selected, setSelected] = useState<DisplayRow | null>(null)
  const [inspectorTab, setInspectorTab] = useState<'edit' | 'rule'>('edit')

  // Top manual injector drawer
  const [showInjector, setShowInjector] = useState(false)
  const [injectType, setInjectType] = useState('permission.asked')
  const [injectDir, setInjectDir] = useState('')
  const [applyShadow, setApplyShadow] = useState(true)
  const [injectJson, setInjectJson] = useState(
    JSON.stringify(
      {
        type: 'permission.asked',
        properties: {
          id: 'perm_mock_001',
          sessionID: 'ses_demo_replay_001',
          permission: 'bash',
          patterns: ['cargo test --workspace'],
          metadata: { command: 'cargo test --workspace' },
          always: ['cargo test*'],
        },
      },
      null,
      2,
    ),
  )

  // Selected frame manual editor + rule builder state
  const [editPayloadText, setEditPayloadText] = useState('')
  const [editDirectory, setEditDirectory] = useState('')
  const [editApplyShadow, setEditApplyShadow] = useState(true)
  const [mutatePrompt, setMutatePrompt] = useState('Mutate fields to simulate tool error or permission prompt')
  const [mutating, setMutating] = useState(false)

  const [ruleName, setRuleName] = useState('')
  const [rulePattern, setRulePattern] = useState('')
  const [ruleSession, setRuleSession] = useState('')
  const [ruleContains, setRuleContains] = useState('')
  const [ruleAction, setRuleAction] = useState<SseMockActionType>('replace')
  const [ruleDelayMs, setRuleDelayMs] = useState(0)
  const [ruleOneShot, setRuleOneShot] = useState(false)

  const activeRecord: SseRecord | null = useMemo(() => {
    if (!selected) return null
    return selected.kind === 'single' ? selected.record : selected.records[selected.records.length - 1] ?? null
  }, [selected])

  useEffect(() => {
    if (!activeRecord) return
    const formatted = JSON.stringify(activeRecord.payload, null, 2)
    setEditPayloadText(formatted)
    setEditDirectory(activeRecord.directory === 'global' ? '' : activeRecord.directory)
    setRuleName(`SSE ${activeRecord.event_type}`)
    setRulePattern(activeRecord.event_type)
    setRuleSession('')
    setRuleContains('')
    setRuleAction('replace')
    setRuleDelayMs(0)
    setRuleOneShot(activeRecord.event_type === 'message.part.delta')
  }, [activeRecord?.seq])

  const filtered = useMemo(() => {
    const q = filterText.trim().toLowerCase()
    const base = props.events.filter((e) => {
      if (!q) return true
      return (
        e.event_type.toLowerCase().includes(q) ||
        (e.session_id ?? '').toLowerCase().includes(q) ||
        e.directory.toLowerCase().includes(q) ||
        JSON.stringify(e.payload).toLowerCase().includes(q)
      )
    })
    return foldDeltas ? foldClientSide(base) : base.map((record): DisplayRow => ({ kind: 'single', record }))
  }, [props.events, filterText, foldDeltas])

  const generateTemplate = async (evType: string) => {
    setInjectType(evType)
    try {
      const data = await api.fakeSse(evType, injectDir || props.activeDirectories[0])
      setInjectJson(JSON.stringify(data, null, 2))
    } catch (e) {
      props.onNotify(String(e), true)
    }
  }

  const sendInject = async (rawJson: string, dirOverride: string, shadowFlag: boolean) => {
    let payload: unknown
    try {
      payload = JSON.parse(rawJson)
    } catch {
      props.onNotify('SSE payload is not valid JSON', true)
      return
    }
    try {
      await api.injectSse({
        directory: dirOverride || props.activeDirectories[0],
        payload,
        apply_to_shadow: shadowFlag,
      })
      props.onNotify('Emitted edited SSE event to connected clients')
    } catch (e) {
      props.onNotify(String(e), true)
    }
  }

  const runMutateSelected = async () => {
    let parsed: unknown
    try {
      parsed = JSON.parse(editPayloadText)
    } catch {
      props.onNotify('Editor JSON must be valid before running AI Mutate', true)
      return
    }
    setMutating(true)
    try {
      const res = await api.llmMutate({ payload: parsed, instruction: mutatePrompt })
      setEditPayloadText(JSON.stringify(res.data, null, 2))
      props.onNotify('AI mutation applied — tweak manually or click Emit Edited Event')
    } catch (e) {
      props.onNotify(String(e), true)
    } finally {
      setMutating(false)
    }
  }

  const saveSseRuleFromInspector = async () => {
    if (!rulePattern.trim()) {
      props.onNotify('Event type pattern is required', true)
      return
    }
    let parsedJson: unknown = undefined
    if (ruleAction === 'patch' || ruleAction === 'replace') {
      try {
        parsedJson = JSON.parse(editPayloadText)
      } catch {
        props.onNotify('Edited SSE JSON is not valid JSON', true)
        return
      }
    }
    try {
      await api.createSseRule({
        name: ruleName.trim() || `${ruleAction} ${rulePattern.trim()}`,
        event_type_pattern: rulePattern.trim(),
        session_filter: ruleSession.trim() || null,
        directory_filter: editDirectory.trim() || null,
        payload_contains: ruleContains.trim() || null,
        action_type: ruleAction,
        delay_ms: ruleDelayMs,
        patch: ruleAction === 'patch' ? parsedJson : undefined,
        replace_events:
          ruleAction === 'replace'
            ? [{ delay_ms: 0, payload: parsedJson, apply_to_shadow: editApplyShadow }]
            : [],
        one_shot: ruleOneShot,
      })
      props.onSseRulesChanged()
      props.onNotify(`Saved SSE ${ruleAction} rule for ${rulePattern.trim()}`)
    } catch (e) {
      props.onNotify(String(e), true)
    }
  }

  return (
    <>
      <div className="toolbar">
        <div className="toolbar-group">
          <input
            className="input"
            style={{ width: 280 }}
            placeholder="Filter event type, session, payload..."
            value={filterText}
            onChange={(e) => setFilterText(e.target.value)}
          />
          <button
            className={`btn btn-sm ${foldDeltas ? 'btn-primary' : ''}`}
            onClick={() => setFoldDeltas((v) => !v)}
          >
            <Layers size={12} /> {foldDeltas ? 'Deltas Folded' : 'Raw Frames'}
          </button>
        </div>
        <div className="toolbar-group">
          <span className="mono" style={{ color: 'var(--text-muted)', fontSize: 11.5 }}>
            {filtered.length} rows ({props.events.length} raw frames · {props.sseRules.filter((r) => r.enabled).length} active SSE rules)
          </span>
          <button
            className={`btn btn-sm ${showInjector ? 'btn-primary' : ''}`}
            onClick={() => setShowInjector((v) => !v)}
          >
            <Send size={12} /> Custom SSE Injector
          </button>
        </div>
      </div>

      {showInjector && (
        <div style={{ padding: '10px 14px', borderBottom: '1px solid var(--border-subtle)', background: 'var(--bg-surface)' }}>
          <div className="toolbar-group" style={{ marginBottom: 8 }}>
            <select
              className="select"
              value={injectType}
              onChange={(e) => generateTemplate(e.target.value)}
            >
              {(props.openapi?.events ?? []).map((ev) => (
                <option key={ev.event_type} value={ev.event_type}>
                  {ev.event_type}
                </option>
              ))}
            </select>
            <button className="btn btn-sm" onClick={() => generateTemplate(injectType)}>
              <Sparkles size={12} /> Fake from OpenAPI
            </button>
            <input
              className="input"
              style={{ width: 220 }}
              placeholder={props.activeDirectories[0] ?? '/workspace/project'}
              value={injectDir}
              onChange={(e) => setInjectDir(e.target.value)}
            />
            <label style={{ display: 'inline-flex', alignItems: 'center', gap: 5, fontSize: 12, color: 'var(--text-secondary)' }}>
              <input type="checkbox" checked={applyShadow} onChange={(e) => setApplyShadow(e.target.checked)} />
              Update ShadowStore
            </label>
            <button className="btn btn-sm btn-primary" onClick={() => sendInject(injectJson, injectDir, applyShadow)}>
              <Send size={12} /> Emit to Clients
            </button>
          </div>
          <textarea
            className="textarea"
            rows={7}
            value={injectJson}
            onChange={(e) => setInjectJson(e.target.value)}
          />
        </div>
      )}

      <div className="split-pane">
        <div className="pane-left">
          {filtered.length === 0 ? (
            <div className="empty-state">
              No SSE events captured yet. Connect an upstream OpenCode server, inject an event, or play a Session Tape.
            </div>
          ) : (
            <table className="data-table">
              <thead>
                <tr>
                  <th>Seq</th>
                  <th>Source / Flags</th>
                  <th>Event Type</th>
                  <th>Session</th>
                  <th>Directory</th>
                  <th>Preview</th>
                </tr>
              </thead>
              <tbody>
                {filtered.map((row) => {
                  const key = row.kind === 'single' ? `s-${row.record.seq}` : `f-${row.from_seq}-${row.to_seq}`
                  const isSel =
                    selected &&
                    ((row.kind === 'single' && selected.kind === 'single' && row.record.seq === selected.record.seq) ||
                      (row.kind === 'folded' && selected.kind === 'folded' && row.from_seq === selected.from_seq))

                  if (row.kind === 'folded') {
                    return (
                      <tr key={key} className={isSel ? 'selected' : ''} onClick={() => setSelected(row)}>
                        <td className="mono" style={{ color: 'var(--text-muted)' }}>
                          #{row.from_seq}..{row.to_seq}
                        </td>
                        <td>
                          <span className="badge badge-info">{row.source}</span>
                        </td>
                        <td>
                          <span className="badge badge-ok">delta × {row.count}</span>
                        </td>
                        <td className="mono">{row.session_id ?? '—'}</td>
                        <td className="mono" style={{ color: 'var(--text-muted)' }}>
                          {row.directory}
                        </td>
                        <td className="mono" style={{ color: 'var(--text-secondary)' }}>
                          {row.preview}
                        </td>
                      </tr>
                    )
                  }

                  const r = row.record
                  const srcBadge =
                    r.source === 'replay' ? 'badge-warn' : r.source === 'injected' ? 'badge-info' : 'badge-neutral'
                  return (
                    <tr
                      key={key}
                      className={isSel ? 'selected' : ''}
                      style={r.dropped ? { opacity: 0.65 } : undefined}
                      onClick={() => setSelected(row)}
                    >
                      <td className="mono" style={{ color: 'var(--text-muted)' }}>
                        #{r.seq}
                      </td>
                      <td>
                        <span className={`badge ${srcBadge}`}>{r.source}</span>
                        {r.dropped ? (
                          <span className="badge badge-err" style={{ marginLeft: 4 }}>
                            DROPPED
                          </span>
                        ) : r.mocked ? (
                          <span className="badge badge-warn" style={{ marginLeft: 4 }}>
                            MOCK
                          </span>
                        ) : null}
                      </td>
                      <td className="mono" style={{ fontWeight: 500, color: 'var(--accent-strong)' }}>
                        {r.event_type}
                      </td>
                      <td className="mono">{r.session_id ?? '—'}</td>
                      <td className="mono" style={{ color: 'var(--text-muted)' }}>
                        {r.directory}
                      </td>
                      <td className="mono" style={{ color: 'var(--text-secondary)' }}>
                        {JSON.stringify(r.payload).slice(0, 120)}
                      </td>
                    </tr>
                  )
                })}
              </tbody>
            </table>
          )}
        </div>

        {selected && activeRecord && (
          <aside className="pane-right" style={{ width: '50%', maxWidth: 740 }}>
            <div className="inspector-header">
              <span className="mono" style={{ fontWeight: 600 }}>
                {selected.kind === 'single'
                  ? `#${selected.record.seq} ${selected.record.event_type}`
                  : `#${selected.from_seq}..#${selected.to_seq} Folded Deltas (${selected.count} frames)`}
              </span>
              <button className="btn btn-sm btn-ghost" onClick={() => setSelected(null)}>
                <X size={14} />
              </button>
            </div>

            <div className="tab-strip">
              <button
                className={`tab-btn ${inspectorTab === 'edit' ? 'active' : ''}`}
                onClick={() => setInspectorTab('edit')}
              >
                Manual Edit &amp; Re-emit
              </button>
              <button
                className={`tab-btn ${inspectorTab === 'rule' ? 'active' : ''}`}
                onClick={() => setInspectorTab('rule')}
              >
                Save as SSE Mock / Breakpoint Rule
              </button>
            </div>

            <div className="inspector-body">
              {selected.kind === 'folded' && (
                <div className="panel" style={{ padding: 10 }}>
                  <div style={{ fontSize: 11, color: 'var(--text-muted)', marginBottom: 4 }}>
                    Concatenated delta text ({selected.records.length} frames, part {selected.part_id})
                  </div>
                  <pre className="code-block" style={{ maxHeight: 140 }}>
                    {selected.records
                      .map((r) => (r.payload as { properties?: { delta?: string } })?.properties?.delta ?? '')
                      .join('')}
                  </pre>
                </div>
              )}

              {inspectorTab === 'edit' && (
                <>
                  <div className="toolbar-group" style={{ justifyContent: 'space-between' }}>
                    <div className="toolbar-group">
                      <input
                        className="input mono"
                        style={{ width: 210 }}
                        placeholder={props.activeDirectories[0] ?? 'Target directory'}
                        value={editDirectory}
                        onChange={(e) => setEditDirectory(e.target.value)}
                      />
                      <label style={{ display: 'inline-flex', alignItems: 'center', gap: 4, fontSize: 11.5 }}>
                        <input
                          type="checkbox"
                          checked={editApplyShadow}
                          onChange={(e) => setEditApplyShadow(e.target.checked)}
                        />
                        ShadowStore
                      </label>
                    </div>
                    <div className="toolbar-group" style={{ gap: 4 }}>
                      <button
                        className="btn btn-sm"
                        onClick={() => {
                          try {
                            setEditPayloadText(JSON.stringify(JSON.parse(editPayloadText), null, 2))
                          } catch {
                            props.onNotify('Not valid JSON', true)
                          }
                        }}
                      >
                        Format
                      </button>
                      <button
                        className="btn btn-sm"
                        onClick={() => setEditPayloadText(JSON.stringify(activeRecord.payload, null, 2))}
                        title="Reset to captured frame"
                      >
                        <RotateCcw size={11} /> Reset
                      </button>
                      <button
                        className="btn btn-sm btn-primary"
                        onClick={() => sendInject(editPayloadText, editDirectory, editApplyShadow)}
                      >
                        <Send size={11} /> Emit Edited Event
                      </button>
                    </div>
                  </div>

                  <div className="field">
                    <label>Editable SSE Event Payload JSON</label>
                    <textarea
                      className="textarea"
                      rows={13}
                      value={editPayloadText}
                      onChange={(e) => setEditPayloadText(e.target.value)}
                    />
                  </div>

                  <div className="panel" style={{ padding: 10 }}>
                    <div style={{ fontSize: 11.5, color: 'var(--text-secondary)', marginBottom: 6 }}>
                      Optional AI Helper (mutates editor JSON above for further manual editing)
                    </div>
                    <div style={{ display: 'flex', gap: 6 }}>
                      <input
                        className="input"
                        style={{ flex: 1 }}
                        value={mutatePrompt}
                        onChange={(e) => setMutatePrompt(e.target.value)}
                      />
                      <button className="btn btn-sm" disabled={mutating} onClick={runMutateSelected}>
                        <Sparkles size={12} /> {mutating ? 'Mutating...' : 'AI Mutate'}
                      </button>
                    </div>
                  </div>
                </>
              )}

              {inspectorTab === 'rule' && (
                <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
                  <div className="form-grid">
                    <div className="field">
                      <label>Rule Name</label>
                      <input className="input" value={ruleName} onChange={(e) => setRuleName(e.target.value)} />
                    </div>
                    <div className="field">
                      <label>Action</label>
                      <select
                        className="select"
                        value={ruleAction}
                        onChange={(e) => setRuleAction(e.target.value as SseMockActionType)}
                      >
                        <option value="replace">Replace with Edited Payload Below</option>
                        <option value="patch">JSON Merge Patch with Payload Below</option>
                        <option value="drop">Drop / Suppress Matching Events</option>
                        <option value="delay">Artificial Delay Only</option>
                        <option value="breakpoint">Suspend Live SSE Breakpoint</option>
                      </select>
                    </div>
                  </div>

                  <div className="form-grid">
                    <div className="field">
                      <label>Event Type Pattern (* or regex:)</label>
                      <input
                        className="input mono"
                        value={rulePattern}
                        onChange={(e) => setRulePattern(e.target.value)}
                      />
                    </div>
                    <div className="field">
                      <label>Session Filter (optional)</label>
                      <input
                        className="input mono"
                        placeholder={activeRecord.session_id ?? 'Any session'}
                        value={ruleSession}
                        onChange={(e) => setRuleSession(e.target.value)}
                      />
                    </div>
                    <div className="field">
                      <label>Payload Contains (optional)</label>
                      <input
                        className="input mono"
                        placeholder="Keyword or regex:..."
                        value={ruleContains}
                        onChange={(e) => setRuleContains(e.target.value)}
                      />
                    </div>
                    <div className="field">
                      <label>Delay (ms)</label>
                      <input
                        className="input mono"
                        type="number"
                        value={ruleDelayMs}
                        onChange={(e) => setRuleDelayMs(Number(e.target.value))}
                      />
                    </div>
                  </div>

                  <label style={{ display: 'inline-flex', alignItems: 'center', gap: 6, fontSize: 12 }}>
                    <input
                      type="checkbox"
                      checked={ruleOneShot}
                      onChange={(e) => setRuleOneShot(e.target.checked)}
                    />
                    One-shot (auto-disable after 1 hit)
                  </label>

                  {(ruleAction === 'replace' || ruleAction === 'patch') && (
                    <div className="field">
                      <label>
                        {ruleAction === 'replace'
                          ? 'Replacement SSE Event JSON'
                          : 'JSON Merge Patch Object'}
                      </label>
                      <textarea
                        className="textarea"
                        rows={10}
                        value={editPayloadText}
                        onChange={(e) => setEditPayloadText(e.target.value)}
                      />
                    </div>
                  )}

                  <div className="toolbar-group">
                    <button className="btn btn-primary" onClick={saveSseRuleFromInspector}>
                      <Save size={13} /> Save SSE Rule
                    </button>
                    {activeRecord.session_id && !ruleSession && (
                      <button
                        className="btn btn-sm"
                        onClick={() => setRuleSession(activeRecord.session_id ?? '')}
                      >
                        <Wand2 size={11} /> Pin to Session {activeRecord.session_id}
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
