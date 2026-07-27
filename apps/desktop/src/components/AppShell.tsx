import type { ReactNode } from "react";
import type { AgentUiMetadataView, AgentView, ServeView } from "../api";
import { AgentIcon } from "../brandIcons";
import { useLanguage } from "./LanguageProvider";

export type AppView =
  | "home"
  | "usage"
  | "settings"
  | "add-provider"
  | `free-provider:${string}`
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
  const { t } = useLanguage();
  const scanned = new Map(agents.map((agent) => [agent.metadata.agent_id, agent]));
  const runtimeHealthy = serve.app_runtime === "running" && serve.listener_reachable;
  const taskRunning = serve.app_runtime === "running";
  const serveLabel =
    taskRunning && !serve.listener_reachable
      ? t("serve.unknown")
      : runtimeHealthy
        ? t("serve.running")
      : serve.phase === "starting"
        ? t("serve.starting")
        : serve.phase === "stopping"
          ? t("serve.stopping")
          : serve.phase === "error"
            ? t("serve.retry")
            : t("serve.startProxy");

  return (
    <div className="station-shell">
      <aside className="station-rail" aria-label={t("nav.main")}>
        <button className="station-brand" type="button" onClick={() => onNavigate("home")} aria-label={t("nav.homeLabel")}>
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
            <span>{t("nav.home")}</span>
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
                <NavGlyph>
                  <AgentIcon
                    id={metadata.agent_id}
                    fallback={metadata.nav_mark ?? metadata.display_name.slice(0, 1)}
                    size={22}
                  />
                </NavGlyph>
                <span className={`agent-status-dot ${agentTone(agent)}`} aria-hidden="true" />
                <span>{metadata.display_name.replace(" Agent", "")}</span>
              </button>
            );
          })}
        </nav>

        <button className="rail-rescan" type="button" disabled={scanBusy} onClick={onRescan}>
          <span aria-hidden="true">↻</span>
          <span>{scanBusy ? t("nav.scanning") : t("nav.rescan")}</span>
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
                {t("serve.agent")}：{serve.agent_connected ? t("serve.connected") : t("serve.disconnected")}
              </small>
            </span>
            <button
              className={`btn compact ${taskRunning ? "" : "primary"}`}
              type="button"
              disabled={commandBusy || serve.phase === "stopping"}
              onClick={onToggleServe}
            >
              {taskRunning
                ? t("serve.stop")
                : serve.phase === "starting"
                  ? t("serve.cancel")
                  : serve.phase === "stopping"
                    ? t("serve.stoppingShort")
                    : t("serve.start")}
            </button>
          </div>

          <div className="top-actions">
            <button
              className={`icon-action ${view === "usage" ? "active" : ""}`}
              type="button"
              onClick={() => onNavigate("usage")}
              aria-label={t("nav.usage")}
              title={t("nav.usage")}
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
              aria-label={t("nav.settings")}
              title={t("nav.settings")}
            >
              <span aria-hidden="true">⚙</span>
            </button>
            {view !== "add-provider" && !view.startsWith("free-provider") && (
              <button className="btn primary add-provider-action" type="button" onClick={() => onNavigate("add-provider")}>
                <span aria-hidden="true">＋</span> {t("nav.addProvider")}
              </button>
            )}
          </div>
        </header>

        <main className="station-content">{children}</main>
      </div>
    </div>
  );
}
