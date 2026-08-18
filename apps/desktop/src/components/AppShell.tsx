import type { ReactNode } from "react";
import { Activity, Settings } from "lucide-react";
import type { AgentUiMetadataView, AgentView, ServeView } from "../api";
import { useLanguage } from "./LanguageProvider";
import TokenStationMark from "./TokenStationMark";
import { Button } from "./ui/button";

export type AppView =
  | "overview"
  | "home"
  | "agents"
  | "providers"
  | "usage"
  | "logs"
  | "usage-management"
  | "quota-usage"
  | "settings"
  | "add-provider"
  | `free-provider:${string}`
  | `agent:${string}`
  | `agent-route:${string}`;

interface AppShellProps {
  view: AppView;
  serve: ServeView;
  registry: AgentUiMetadataView[];
  agents: AgentView[];
  commandBusy: boolean;
  discoveryPending?: boolean;
  onNavigate: (view: AppView) => void;
  onToggleServe: () => void;
  children: ReactNode;
}

const PRIMARY_NAV: Array<{ view: AppView; en: string; zh: string }> = [
  { view: "overview", en: "Home", zh: "主页" },
  { view: "agents", en: "Agent", zh: "Agent" },
  { view: "home", en: "Routing", zh: "路由" },
  { view: "providers", en: "Providers", zh: "供应商" },
  { view: "usage", en: "Usage", zh: "用量" },
];

function primaryView(view: AppView): AppView {
  if (view.startsWith("agent:")) return "agents";
  if (view.startsWith("agent-route:")) return "home";
  if (view === "logs" || view === "settings") return "settings";
  if (view === "quota-usage" || view === "usage-management") return "usage";
  if (view === "add-provider" || view.startsWith("free-provider:")) return "providers";
  return view;
}

export default function AppShell({
  view,
  serve,
  registry,
  agents,
  commandBusy,
  discoveryPending = false,
  onNavigate,
  onToggleServe,
  children,
}: AppShellProps) {
  const { t, copy } = useLanguage();
  const runtimeHealthy = serve.app_runtime === "running" && serve.listener_reachable;
  const taskRunning = serve.app_runtime === "running";
  const activePrimary = primaryView(view);
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
  const connectedAgents = agents.filter((agent) => agent.status === "CONNECTED").length;

  return (
    <div className="station-shell station-shell-topnav">
      <header className="station-header">
        <button
          data-onboarding-return-focus
          data-onboarding-target="overview-entry"
          className="station-brand-top"
          type="button"
          disabled={commandBusy}
          onClick={() => onNavigate("overview")}
          aria-label={copy("Token Station Overview", "Token Station 概览")}
        >
          <TokenStationMark className="station-brand-mark" size={28} />
          <span>Token Station</span>
        </button>

        <nav className="station-primary-nav" aria-label={t("nav.main")}>
          {PRIMARY_NAV.map((item) => {
            const label = copy(item.en, item.zh);
            const selected = activePrimary === item.view;
            return (
              <Button
                key={item.view}
                className="station-primary-link"
                variant="ghost"
                size="sm"
                type="button"
                disabled={discoveryPending || (commandBusy && !selected)}
                aria-current={selected ? "page" : undefined}
                aria-label={label}
                data-onboarding-target={
                  item.view === "home"
                    ? "home-entry"
                    : item.view === "agents"
                      ? "agent-entry"
                    : undefined
                }
                onClick={() => onNavigate(item.view)}
              >
                <span data-onboarding-target={item.view === "settings" ? "settings" : undefined}>
                  {label}
                </span>
                {item.view === "agents" && agents.length > connectedAgents && (
                  <span className="station-nav-alert" aria-hidden="true" />
                )}
              </Button>
            );
          })}
        </nav>

        <div className="station-header-actions">
          <Button
            className={`station-runtime-pill ${runtimeHealthy ? "healthy" : ""}`}
            variant="outline"
            size="lg"
            type="button"
            disabled={commandBusy || serve.phase === "stopping"}
            onClick={onToggleServe}
            aria-label={`${serveLabel} · ${serve.listen} · ${taskRunning ? t("serve.stop") : t("serve.start")}`}
            title={`${serveLabel} · ${serve.listen}`}
          >
            <Activity aria-hidden="true" />
            <span className="station-runtime-copy"><strong>{serveLabel}</strong><small>{serve.listen}</small></span>
            {serve.running_revision != null && <code>rev {serve.running_revision}</code>}
          </Button>
          <Button
            className="station-settings-button"
            data-onboarding-target="settings"
            variant="ghost"
            size="icon-lg"
            type="button"
            disabled={commandBusy}
            aria-current={activePrimary === "settings" ? "page" : undefined}
            aria-label={t("nav.settings")}
            title={t("nav.settings")}
            onClick={() => onNavigate("settings")}
          >
            <Settings aria-hidden="true" />
          </Button>
        </div>
      </header>

      <main className={`station-content station-content-topnav${activePrimary === "home" || activePrimary === "agents" ? " station-content-agent" : ""}`}>
        {children}
      </main>

      {!discoveryPending && (
        <span className="station-agent-summary" data-testid="agent-runtime-connection" aria-live="polite">
          {copy(
            `Agent: ${serve.agent_connected ? "Connected" : "Disconnected"}`,
            `Agent：${serve.agent_connected ? "已连接" : "未连接"}`,
          )}
          {copy(
            ` · ${connectedAgents} of ${registry.length} managed`,
            ` · ${connectedAgents} / ${registry.length} 个已接管`,
          )}
        </span>
      )}
    </div>
  );
}
