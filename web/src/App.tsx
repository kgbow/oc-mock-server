import { useCallback, useEffect, useRef, useState } from 'react'
import {
  Activity,
  AlertTriangle,
  Circle,
  Cpu,
  Network,
  PauseCircle,
  PlayCircle,
  Radio,
  Trash2,
  Volume2,
  VolumeX,
  Wand2,
} from 'lucide-react'
import { api } from './api'
import { AnomaliesView } from './components/AnomaliesView'
import { BreakpointsView } from './components/BreakpointsView'
import { ConfigMcpView } from './components/ConfigMcpView'
import { MockStudioView } from './components/MockStudioView'
import { NetworkView } from './components/NetworkView'
import { ReplayView } from './components/ReplayView'
import { SseView } from './components/SseView'
import type {
  Anomaly,
  ControlWsEvent,
  HttpTraceSummary,
  MockRule,
  OpenApiSummary,
  PendingBreakpoint,
  ProxyMode,
  ProxyRuntimeStatus,
  SessionOverview,
  SseMockRule,
  SseRecord,
  TapeSummary,
} from './types'

type NavTab = 'network' | 'sse' | 'mock' | 'breakpoints' | 'replay' | 'anomalies' | 'config'

const MODE_TITLES: Record<ProxyMode, string> = {
  passthrough: 'Pure Passthrough: captures traffic and bypasses all HTTP/SSE mock & breakpoint rules',
  intercept: 'Active Interception: evaluates all enabled HTTP/SSE mock rules and live breakpoints',
  replay: 'Session Replay: mounts ShadowStore virtual sessions and mutes upstream SSE',
}

export default function App() {
  const [tab, setTab] = useState<NavTab>('network')
  const [wsConnected, setWsConnected] = useState(false)
  const [status, setStatus] = useState<ProxyRuntimeStatus | null>(null)
  const [traces, setTraces] = useState<HttpTraceSummary[]>([])
  const [sseEvents, setSseEvents] = useState<SseRecord[]>([])
  const [rules, setRules] = useState<MockRule[]>([])
  const [sseRules, setSseRules] = useState<SseMockRule[]>([])
  const [breakpoints, setBreakpoints] = useState<PendingBreakpoint[]>([])
  const [tapes, setTapes] = useState<TapeSummary[]>([])
  const [sessions, setSessions] = useState<SessionOverview[]>([])
  const [anomalies, setAnomalies] = useState<Anomaly[]>([])
  const [openapi, setOpenapi] = useState<OpenApiSummary | null>(null)
  const [toast, setToast] = useState<{ msg: string; isErr: boolean } | null>(null)
  const toastTimer = useRef<number | null>(null)
  const tabRef = useRef<NavTab>('network')
  tabRef.current = tab

  const notify = useCallback((msg: string, isErr = false) => {
    setToast({ msg, isErr })
    if (toastTimer.current) window.clearTimeout(toastTimer.current)
    toastTimer.current = window.setTimeout(() => setToast(null), 4000)
  }, [])

  const loadRules = useCallback(() => {
    api.listRules().then(setRules).catch(() => {})
  }, [])

  const loadSseRules = useCallback(() => {
    api.listSseRules().then(setSseRules).catch(() => {})
  }, [])

  const loadTapes = useCallback(() => {
    api.listTapes().then(setTapes).catch(() => {})
  }, [])

  const loadBreakpoints = useCallback(() => {
    api.listBreakpoints().then(setBreakpoints).catch(() => {})
  }, [])

  const loadOpenApi = useCallback(() => {
    api.getOpenApi().then(setOpenapi).catch(() => {})
  }, [])

  const loadDiagnostics = useCallback(() => {
    api.listSessions().then(setSessions).catch(() => {})
    api.listAnomalies().then(setAnomalies).catch(() => {})
  }, [])

  const loadInitial = useCallback(() => {
    api.getStatus().then(setStatus).catch(() => {})
    api.listHttp({ limit: 300 }).then(setTraces).catch(() => {})
    api
      .listSse({ limit: 400 })
      .then((list) => setSseEvents(list as SseRecord[]))
      .catch(() => {})
    loadRules()
    loadSseRules()
    loadBreakpoints()
    loadTapes()
    loadOpenApi()
    loadDiagnostics()
  }, [loadRules, loadSseRules, loadBreakpoints, loadTapes, loadOpenApi, loadDiagnostics])

  useEffect(() => {
    loadInitial()
  }, [loadInitial])

  // Live WebSocket stream to Control Plane
  useEffect(() => {
    let closed = false
    let ws: WebSocket | null = null
    let reconnectTimer: number | null = null

    const connect = () => {
      if (closed) return
      const proto = window.location.protocol === 'https:' ? 'wss:' : 'ws:'
      ws = new WebSocket(`${proto}//${window.location.host}/api/ws`)
      ws.onopen = () => setWsConnected(true)
      ws.onclose = () => {
        setWsConnected(false)
        if (!closed) {
          reconnectTimer = window.setTimeout(connect, 1500)
        }
      }
      ws.onmessage = (ev) => {
        try {
          const msg = JSON.parse(ev.data) as ControlWsEvent
          switch (msg.type) {
            case 'status':
              setStatus(msg.status)
              break
            case 'http_started':
              setTraces((prev) => [msg.trace, ...prev].slice(0, 500))
              break
            case 'http_completed':
              setTraces((prev) => {
                const idx = prev.findIndex((t) => t.id === msg.trace.id)
                if (idx === -1) return [msg.trace, ...prev].slice(0, 500)
                const next = [...prev]
                next[idx] = msg.trace
                return next
              })
              break
            case 'sse_recorded':
              setSseEvents((prev) => [msg.event, ...prev].slice(0, 800))
              break
            case 'breakpoint_added':
              setBreakpoints((prev) => [...prev.filter((b) => b.id !== msg.breakpoint.id), msg.breakpoint])
              if (tabRef.current !== 'breakpoints') {
                notify(`Breakpoint suspended: ${msg.breakpoint.method} ${msg.breakpoint.path}`)
              }
              break
            case 'breakpoint_resolved':
              setBreakpoints((prev) => prev.filter((b) => b.id !== msg.id))
              break
            case 'replay_updated':
              setStatus((prev) => (prev ? { ...prev, replay: msg.replay } : prev))
              break
            case 'rules_updated':
              loadRules()
              break
            case 'sse_rules_updated':
              loadSseRules()
              break
            case 'tapes_updated':
              loadTapes()
              break
          }
        } catch {}
      }
    }

    connect()
    return () => {
      closed = true
      if (reconnectTimer) window.clearTimeout(reconnectTimer)
      ws?.close()
    }
  }, [loadRules, loadSseRules, loadTapes, notify])

  const changeMode = async (mode: ProxyMode) => {
    try {
      const next = await api.setMode(mode)
      setStatus(next)
      if (mode === 'passthrough') {
        notify('Switched to Passthrough — all mock & breakpoint rules are temporarily bypassed')
      } else if (mode === 'intercept') {
        notify('Switched to Intercept — enabled HTTP & SSE rules/breakpoints are active')
      } else {
        notify('Switched to Replay — ShadowStore active & upstream SSE muted')
      }
    } catch (e) {
      notify(String(e), true)
    }
  }

  const toggleRecording = async () => {
    if (!status) return
    try {
      setStatus(await api.setRecording(!status.recording))
    } catch (e) {
      notify(String(e), true)
    }
  }

  const toggleMute = async () => {
    if (!status) return
    try {
      setStatus(await api.setMuteSse(!status.mute_upstream_sse))
    } catch (e) {
      notify(String(e), true)
    }
  }

  const clearAll = async () => {
    try {
      await api.clearTraffic()
      setTraces([])
      setSseEvents([])
      setAnomalies([])
      setSessions([])
      notify('Cleared captured traffic')
    } catch (e) {
      notify(String(e), true)
    }
  }

  const enabledRuleCount =
    rules.filter((r) => r.enabled).length + sseRules.filter((r) => r.enabled).length
  const totalRuleCount = rules.length + sseRules.length

  return (
    <div className="app-shell">
      <header className="topbar">
        <div className="topbar-left">
          <div className="brand">
            <span className="brand-mark">
              <Activity size={13} />
            </span>
            <span>oc-mock-server</span>
          </div>

          <div className="route-chip mono" title={status?.upstream_url ?? 'Offline standalone mode'}>
            <span className={`status-dot ${wsConnected ? 'ok' : 'err'}`} />
            <span>:{status?.proxy_addr.split(':').pop() ?? '4097'}</span>
            <span>→</span>
            <span>{status?.upstream_url ?? 'offline'}</span>
          </div>

          <div className="segmented" role="group" aria-label="Proxy mode">
            {(['passthrough', 'intercept', 'replay'] as ProxyMode[]).map((m) => (
              <button
                key={m}
                className={status?.mode === m ? 'active' : ''}
                title={MODE_TITLES[m]}
                onClick={() => changeMode(m)}
              >
                {m === 'passthrough' ? 'Passthrough' : m === 'intercept' ? 'Intercept' : 'Replay'}
              </button>
            ))}
          </div>

          {status?.mode === 'passthrough' && enabledRuleCount > 0 && (
            <button
              className="btn btn-sm btn-warn"
              onClick={() => changeMode('intercept')}
              title="Click to switch to Intercept mode so enabled rules take effect"
            >
              {enabledRuleCount} rule(s) bypassed — Enable Intercept
            </button>
          )}
        </div>

        <div className="topbar-right">
          <button
            className={`btn btn-sm ${status?.recording ? 'btn-danger' : ''}`}
            onClick={toggleRecording}
            title="Toggle traffic recording to SQLite"
          >
            <Circle size={10} fill={status?.recording ? 'currentColor' : 'none'} />
            {status?.recording ? 'Rec' : 'Paused'}
          </button>

          <button
            className={`btn btn-sm ${status?.mute_upstream_sse ? 'btn-warn' : ''}`}
            onClick={toggleMute}
            title="Mute upstream SSE forwarding to clients"
          >
            {status?.mute_upstream_sse ? <VolumeX size={12} /> : <Volume2 size={12} />}
            {status?.mute_upstream_sse ? 'SSE Muted' : 'SSE Live'}
          </button>

          <button className="btn btn-sm" onClick={clearAll} title="Clear captured HTTP and SSE history">
            <Trash2 size={12} /> Clear
          </button>
        </div>
      </header>

      <div className="workspace">
        <nav className="sidebar" aria-label="DevTools sections">
          <div className="nav-section-label">Inspect</div>
          <button className={`nav-item ${tab === 'network' ? 'active' : ''}`} onClick={() => setTab('network')}>
            <span className="nav-item-label">
              <Network size={14} /> Network
            </span>
            <span className="nav-badge">{traces.length}</span>
          </button>
          <button className={`nav-item ${tab === 'sse' ? 'active' : ''}`} onClick={() => setTab('sse')}>
            <span className="nav-item-label">
              <Radio size={14} /> SSE Stream
            </span>
            <span className="nav-badge">{sseEvents.length}</span>
          </button>
          <button
            className={`nav-item ${tab === 'anomalies' ? 'active' : ''}`}
            onClick={() => {
              setTab('anomalies')
              loadDiagnostics()
            }}
          >
            <span className="nav-item-label">
              <AlertTriangle size={14} /> Anomalies
            </span>
            {anomalies.length > 0 && <span className="nav-badge err">{anomalies.length}</span>}
          </button>

          <div className="nav-section-label">Simulate</div>
          <button className={`nav-item ${tab === 'mock' ? 'active' : ''}`} onClick={() => setTab('mock')}>
            <span className="nav-item-label">
              <Wand2 size={14} /> Mock &amp; AI
            </span>
            <span className="nav-badge">{totalRuleCount}</span>
          </button>
          <button className={`nav-item ${tab === 'breakpoints' ? 'active' : ''}`} onClick={() => setTab('breakpoints')}>
            <span className="nav-item-label">
              <PauseCircle size={14} /> Breakpoints
            </span>
            {breakpoints.length > 0 && <span className="nav-badge warn">{breakpoints.length}</span>}
          </button>
          <button
            className={`nav-item ${tab === 'replay' ? 'active' : ''}`}
            onClick={() => {
              setTab('replay')
              loadTapes()
              loadDiagnostics()
            }}
          >
            <span className="nav-item-label">
              <PlayCircle size={14} /> Replay &amp; Tapes
            </span>
            <span className="nav-badge">{tapes.length}</span>
          </button>

          <div className="nav-section-label">Integrate</div>
          <button className={`nav-item ${tab === 'config' ? 'active' : ''}`} onClick={() => setTab('config')}>
            <span className="nav-item-label">
              <Cpu size={14} /> MCP &amp; Config
            </span>
          </button>

          <div className="sidebar-footer mono">
            <div className="sidebar-stat">
              <span>Active Rules</span>
              <span style={{ color: status?.mode === 'passthrough' ? 'var(--text-muted)' : 'var(--ok)' }}>
                {status?.mode === 'passthrough' ? `0 (${enabledRuleCount} paused)` : enabledRuleCount}
              </span>
            </div>
            <div className="sidebar-stat">
              <span>SSE Clients</span>
              <span style={{ color: 'var(--text-primary)' }}>{status?.connected_sse_clients ?? 0}</span>
            </div>
            <div className="sidebar-stat">
              <span>OpenAPI</span>
              <span style={{ color: 'var(--accent-strong)' }}>
                {status?.openapi_source.startsWith('upstream:') ? 'upstream' : 'builtin'}
              </span>
            </div>
            <div className="sidebar-stat">
              <span>MCP HTTP</span>
              <span style={{ color: 'var(--ok)' }}>/mcp</span>
            </div>
          </div>
        </nav>

        <main className="main-area">
          {tab === 'network' && (
            <NetworkView
              traces={traces}
              rules={rules}
              openapi={openapi}
              activeDirectories={status?.active_directories ?? []}
              onNotify={notify}
              onRulesChanged={loadRules}
            />
          )}
          {tab === 'sse' && (
            <SseView
              events={sseEvents}
              sseRules={sseRules}
              openapi={openapi}
              activeDirectories={status?.active_directories ?? []}
              onSseRulesChanged={loadSseRules}
              onNotify={notify}
            />
          )}
          {tab === 'mock' && (
            <MockStudioView
              rules={rules}
              sseRules={sseRules}
              openapi={openapi}
              activeDirectories={status?.active_directories ?? []}
              onRefreshRules={loadRules}
              onRefreshSseRules={loadSseRules}
              onRefreshOpenApi={loadOpenApi}
              onRefreshTapes={loadTapes}
              onNotify={notify}
            />
          )}
          {tab === 'breakpoints' && (
            <BreakpointsView
              breakpoints={breakpoints}
              rules={rules}
              sseRules={sseRules}
              openapi={openapi}
              onRefresh={loadBreakpoints}
              onRefreshRules={loadRules}
              onRefreshSseRules={loadSseRules}
              onNotify={notify}
            />
          )}
          {tab === 'replay' && (
            <ReplayView
              tapes={tapes}
              sessions={sessions}
              openapi={openapi}
              replay={
                status?.replay ?? {
                  state: 'stopped',
                  tape_id: null,
                  tape_name: null,
                  session_id: null,
                  cursor: 0,
                  total_events: 0,
                  speed: 1,
                  min_frame_ms: 20,
                  target_directory: null,
                  mount_shadow: true,
                  web_session_url: null,
                }
              }
              activeDirectories={status?.active_directories ?? []}
              onRefreshTapes={loadTapes}
              onNotify={notify}
            />
          )}
          {tab === 'anomalies' && (
            <AnomaliesView
              anomalies={anomalies}
              sessions={sessions}
              onRefresh={loadDiagnostics}
              onNotify={notify}
            />
          )}
          {tab === 'config' && <ConfigMcpView status={status} onNotify={notify} />}
        </main>
      </div>

      {toast && <div className={`toast ${toast.isErr ? 'err' : 'ok'}`}>{toast.msg}</div>}
    </div>
  )
}
