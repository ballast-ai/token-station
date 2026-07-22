import type { ReactNode } from "react";
import type { AgentUiMetadataView, AgentView, ServeView } from "../api";

export type AppView =
  | "home"
  | "usage"
  | "settings"
  | "add-provider"
  | `agent:${string}`;

interface AppShellProps {
  view: AppView;
  serve: ServeView;
  registry: AgentUiMetadataView[];
  agents: AgentView[];
  scanBusy: boolean;
  commandBusy: boolean;
  onNavigate: (view: AppView) => void;
  onRescan: () => void;
  onToggleServe: () => void;
  children: ReactNode;
}

const AGENT_MARKS: Record<string, string> = {
  "claude-code": "C",
  codex: "X",
  opencode: "O",
  openclaw: "OC",
  "nous-hermes-agent": "H",
};

function agentTone(agent: AgentView | undefined) {
  if (agent?.status === "CONNECTED") return "connected";
  if (agent?.status === "DETECTED_BLOCKED" || agent?.status === "INSTALLED_BROKEN") return "blocked";
  if (agent && agent.installations.length > 0) return "detected";
  return "idle";
}

function NavGlyph({ children }: { children: ReactNode }) {
  return <span className="rail-glyph" aria-hidden="true">{children}</span>;
}

export default function AppShell({
  view,
  serve,
  registry,
  agents,
  scanBusy,
  commandBusy,
  onNavigate,
  onRescan,
  onToggleServe,
  children,
}: AppShellProps) {
  const scanned = new Map(agents.map((agent) => [agent.metadata.agent_id, agent]));
  const runtimeHealthy = serve.app_runtime === "running" && serve.listener_reachable;
  const taskRunning = serve.app_runtime === "running";
  const serveLabel =
    taskRunning && !serve.listener_reachable
      ? "运行态未知"
      : runtimeHealthy
        ? "代理运行中"
      : serve.phase === "starting"
        ? "正在启动"
        : serve.phase === "stopping"
          ? "正在停止"
          : serve.phase === "error"
            ? "重试启动"
            : "启动代理";

  return (
    <div className="station-shell">
      <aside className="station-rail" aria-label="主导航">
        <button className="station-brand" type="button" onClick={() => onNavigate("home")} aria-label="Token Station 主页">
          <span className="brand-signal" aria-hidden="true"><i /><i /><i /></span>
          <span className="brand-word">Token<br />Station</span>
        </button>

        <nav className="rail-nav">
          <button
            className={`rail-item ${view === "home" ? "active" : ""}`}
            type="button"
            onClick={() => onNavigate("home")}
            aria-current={view === "home" ? "page" : undefined}
          >
            <NavGlyph>⌂</NavGlyph>
            <span>主页</span>
          </button>

          <div className="rail-label">AGENTS</div>
          <div className="signal-track" aria-hidden="true" />
          {registry.map((metadata) => {
            const agent = scanned.get(metadata.agent_id);
            const selected = view === `agent:${metadata.agent_id}`;
            return (
              <button
                key={metadata.agent_id}
                className={`rail-item agent-link ${selected ? "active" : ""}`}
                type="button"
                onClick={() => onNavigate(`agent:${metadata.agent_id}`)}
                aria-current={selected ? "page" : undefined}
                title={`${metadata.display_name} · ${agentTone(agent)}`}
              >
                <NavGlyph>{AGENT_MARKS[metadata.agent_id] ?? metadata.display_name.slice(0, 1)}</NavGlyph>
                <span className={`agent-status-dot ${agentTone(agent)}`} aria-hidden="true" />
                <span>{metadata.display_name.replace(" Agent", "")}</span>
              </button>
            );
          })}
        </nav>

        <button className="rail-rescan" type="button" disabled={scanBusy} onClick={onRescan}>
          <span aria-hidden="true">↻</span>
          <span>{scanBusy ? "扫描中" : "重新扫描"}</span>
        </button>
      </aside>

      <div className="station-workspace">
        <header className="station-topbar">
          <div className="serve-cluster">
            <span className={`serve-indicator ${runtimeHealthy ? "on" : ""}`} aria-hidden="true" />
            <span className="serve-copy">
              <strong>{serveLabel}</strong>
              <small>{serve.listen}</small>
              <small data-testid="agent-runtime-connection">
                Agent：{serve.agent_connected ? "已连接" : "未连接"}
              </small>
            </span>
            <button
              className={`btn compact ${taskRunning ? "" : "primary"}`}
              type="button"
              disabled={commandBusy || serve.phase === "stopping"}
              onClick={onToggleServe}
            >
              {taskRunning ? "停止" : serve.phase === "starting" ? "取消" : serve.phase === "stopping" ? "停止中" : "启动"}
            </button>
          </div>

          <div className="top-actions">
            <button
              className={`icon-action ${view === "usage" ? "active" : ""}`}
              type="button"
              onClick={() => onNavigate("usage")}
              aria-label="用量"
              title="用量"
            >
              <svg className="usage-icon" viewBox="0 0 18 18" aria-hidden="true">
                <path d="M3 14.5V9.5M9 14.5V4.5M15 14.5V7" />
              </svg>
              <i className="usage-dot" />
            </button>
            <button
              className={`icon-action ${view === "settings" ? "active" : ""}`}
              type="button"
              onClick={() => onNavigate("settings")}
              aria-label="设置"
              title="设置"
            >
              <span aria-hidden="true">⚙</span>
            </button>
            {view !== "add-provider" && (
              <button className="btn primary add-provider-action" type="button" onClick={() => onNavigate("add-provider")}>
                <span aria-hidden="true">＋</span> 添加供应商
              </button>
            )}
          </div>
        </header>

        <main className="station-content">{children}</main>
      </div>
    </div>
  );
}
