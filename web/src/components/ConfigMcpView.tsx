import { useEffect, useState } from 'react'
import { Copy, Save } from 'lucide-react'
import { api } from '../api'
import type { ProxyRuntimeStatus } from '../types'

export function ConfigMcpView(props: {
  status: ProxyRuntimeStatus | null
  onNotify: (msg: string, isErr?: boolean) => void
}) {
  const [apiBase, setApiBase] = useState('https://api.openai.com/v1')
  const [apiKey, setApiKey] = useState('')
  const [model, setModel] = useState('gpt-4o-mini')
  const [temperature, setTemperature] = useState(0.3)
  const [configured, setConfigured] = useState(false)

  useEffect(() => {
    api
      .getLlmConfig()
      .then((c) => {
        setConfigured(c.configured)
        if (c.api_base) setApiBase(c.api_base)
        if (c.model) setModel(c.model)
        if (c.temperature !== undefined) setTemperature(c.temperature)
      })
      .catch(() => {})
  }, [])

  const saveLlm = async () => {
    try {
      await api.setLlmConfig({ api_base: apiBase, api_key: apiKey, model, temperature })
      setConfigured(true)
      props.onNotify('Saved LLM configuration for AI Mock & Scenario generation')
    } catch (e) {
      props.onNotify(String(e), true)
    }
  }

  const controlAddr = props.status?.control_addr ?? '127.0.0.1:4098'
  const proxyAddr = props.status?.proxy_addr ?? '127.0.0.1:4097'

  const remoteMcpSnippet = JSON.stringify(
    {
      mcp: {
        'oc-mock-server': {
          type: 'remote',
          url: `http://${controlAddr}/mcp`,
          oauth: false,
        },
      },
    },
    null,
    2,
  )

  const localMcpSnippet = JSON.stringify(
    {
      mcp: {
        'oc-mock-server': {
          type: 'local',
          command: ['oc-mock-server', 'mcp', '--db', 'oc-mock.db', '--control-url', `http://${controlAddr}`],
        },
      },
    },
    null,
    2,
  )

  const cliCheatsheet = `# Connect OpenCode TUI to the proxy
opencode attach http://${proxyAddr}

# List captured sessions (JSON for AI agents)
oc-mock-server sessions --json

# Inspect delta-folded unified HTTP + SSE timeline (saves >95% tokens)
oc-mock-server timeline <session_id>

# Auto-diagnose errors, slow calls, orphan deltas, reconnect storms
oc-mock-server anomalies --json

# Full-text FTS5 search across captured HTTP + SSE payloads
oc-mock-server search "UnknownError" --json

# Reconstruct exact client ShadowStore state at SSE sequence #120
oc-mock-server state <session_id> --at-seq 120`

  const copy = (text: string, label: string) => {
    navigator.clipboard?.writeText(text)
    props.onNotify(`Copied ${label}`)
  }

  return (
    <div style={{ flex: 1, overflow: 'auto', padding: 14, display: 'flex', flexDirection: 'column', gap: 14 }}>
      <div className="panel">
        <div className="panel-title">
          <span>LLM Provider Configuration (OpenAI-Compatible)</span>
          <span className={`badge ${configured ? 'badge-ok' : 'badge-neutral'}`}>
            {configured ? 'CONFIGURED' : 'NOT SET'}
          </span>
        </div>
        <div className="form-grid" style={{ marginBottom: 10 }}>
          <div className="field">
            <label>API Base URL (OpenAI / OpenRouter / Ollama)</label>
            <input className="input mono" value={apiBase} onChange={(e) => setApiBase(e.target.value)} />
          </div>
          <div className="field">
            <label>Model ID</label>
            <input className="input mono" value={model} onChange={(e) => setModel(e.target.value)} />
          </div>
          <div className="field">
            <label>API Key (kept in memory only)</label>
            <input
              className="input mono"
              type="password"
              value={apiKey}
              onChange={(e) => setApiKey(e.target.value)}
              placeholder="sk-... (optional for local Ollama)"
            />
          </div>
          <div className="field">
            <label>Temperature</label>
            <input
              className="input mono"
              type="number"
              step={0.1}
              min={0}
              max={1.5}
              value={temperature}
              onChange={(e) => setTemperature(Number(e.target.value))}
            />
          </div>
        </div>
        <button className="btn btn-primary" onClick={saveLlm}>
          <Save size={13} /> Save LLM Settings
        </button>
      </div>

      <div className="panel">
        <div className="panel-title">
          <span>OpenCode Remote MCP Config (Recommended — Live Proxy &amp; Write Tools)</span>
          <button className="btn btn-sm" onClick={() => copy(remoteMcpSnippet, 'Remote MCP config')}>
            <Copy size={12} /> Copy
          </button>
        </div>
        <pre className="code-block">{remoteMcpSnippet}</pre>
      </div>

      <div className="panel">
        <div className="panel-title">
          <span>OpenCode Local Stdio MCP Config</span>
          <button className="btn btn-sm" onClick={() => copy(localMcpSnippet, 'Local MCP config')}>
            <Copy size={12} /> Copy
          </button>
        </div>
        <pre className="code-block">{localMcpSnippet}</pre>
      </div>

      <div className="panel">
        <div className="panel-title">
          <span>CLI &amp; OpenCode Client Cheatsheet</span>
          <button className="btn btn-sm" onClick={() => copy(cliCheatsheet, 'CLI cheatsheet')}>
            <Copy size={12} /> Copy
          </button>
        </div>
        <pre className="code-block">{cliCheatsheet}</pre>
      </div>
    </div>
  )
}
