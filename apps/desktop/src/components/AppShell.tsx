import type { ReactNode } from "react";
import { Activity, Moon, Plus, Route, Sun } from "lucide-react";
import type { AgentUiMetadataView, AgentView, ServeView } from "../api";
import { useLanguage } from "./LanguageProvider";
import { useTheme } from "./ThemeProvider";
import { Button } from "./ui/button";

export type AppView =
  | "overview"
  | "home"
  | "agents"
  | "providers"
  | "usage"
  | "logs"
  | "quota-usage"
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
  routingMode: "tiered" | "quota_first";
  onSetRoutingMode: (mode: "tiered" | "quota_first") => void;
  onNavigate: (view: AppView) => void;
  onRescan: () => void;
  onToggleServe: () => void;
  children: ReactNode;
}

const PRIMARY_NAV: Array<{ view: AppView; en: string; zh: string }> = [
  { view: "overview", en: "Overview", zh: "概览" },
  { view: "home", en: "Routing", zh: "路由" },
  { view: "agents", en: "Agents", zh: "Agent" },
  { view: "providers", en: "Providers", zh: "供应商" },
  { view: "usage", en: "Usage", zh: "用量" },
  { view: "logs", en: "Logs", zh: "日志" },
  { view: "settings", en: "Settings", zh: "设置" },
];

function primaryView(view: AppView): AppView {
  if (view.startsWith("agent:")) return "agents";
  if (view === "quota-usage") return "usage";
  if (view === "add-provider" || view.startsWith("free-provider:")) return "providers";
  return view;
}

export default function AppShell({
  view,
  serve,
  registry,
  agents,
  scanBusy,
  commandBusy,
  routingMode,
  onSetRoutingMode,
  onNavigate,
  onToggleServe,
  children,
}: AppShellProps) {
  const { t, copy } = useLanguage();
  const { resolvedTheme, setTheme } = useTheme();
  const runtimeHealthy = serve.app_runtime === "running" && serve.listener_reachable;
  const taskRunning = serve.app_runtime === "running";
  const activePrimary = primaryView(view);
  const needsBack = view === "add-provider" || view === "quota-usage" || view.startsWith("free-provider:");
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
          className="station-brand-top"
          type="button"
          disabled={commandBusy}
          onClick={() => onNavigate("overview")}
          aria-label={copy("Token Station Overview", "Token Station 概览")}
        >
          <span className="station-brand-mark" aria-hidden="true"><Route /></span>
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
                disabled={commandBusy && !selected}
                aria-current={selected ? "page" : undefined}
                aria-label={label}
                onClick={() => onNavigate(item.view)}
              >
                {label}
                {item.view === "agents" && agents.length > connectedAgents && (
                  <span className="station-nav-alert" aria-hidden="true" />
                )}
              </Button>
            );
          })}
        </nav>

        <div className="station-header-actions">
          {(activePrimary === "home" || activePrimary === "agents") && (
            <div className="station-routing-mode" role="group" aria-label={copy("Routing mode", "路由模式")}>
              <Button
                variant="ghost"
                size="sm"
                type="button"
                aria-pressed={routingMode === "tiered"}
                disabled={commandBusy}
                onClick={() => onSetRoutingMode("tiered")}
              >
                {copy("Smart tiers", "智能分档")}
              </Button>
              <Button
                variant="ghost"
                size="sm"
                type="button"
                aria-pressed={routingMode === "quota_first"}
                disabled={commandBusy}
                onClick={() => onSetRoutingMode("quota_first")}
              >
                {copy("Quota first", "额度优先")}
              </Button>
            </div>
          )}
          <button
            className={`station-runtime-pill ${runtimeHealthy ? "healthy" : ""}`}
            type="button"
            disabled={commandBusy || serve.phase === "stopping"}
            onClick={onToggleServe}
            aria-label={`${serveLabel} · ${taskRunning ? t("serve.stop") : t("serve.start")}`}
            title={`${serveLabel} · ${serve.listen}`}
          >
            <Activity aria-hidden="true" />
            <span>{serveLabel}</span>
            {serve.running_revision != null && <code>rev {serve.running_revision}</code>}
          </button>
          <Button
            variant="ghost"
            size="icon-sm"
            type="button"
            aria-label={copy("Toggle color theme", "切换颜色主题")}
            title={copy("Toggle color theme", "切换颜色主题")}
            onClick={() => setTheme(resolvedTheme === "dark" ? "light" : "dark")}
          >
            {resolvedTheme === "dark" ? <Sun /> : <Moon />}
          </Button>
          {!needsBack && (
            <Button size="sm" type="button" disabled={commandBusy} onClick={() => onNavigate("add-provider")}>
              <Plus />{t("nav.addProvider")}
            </Button>
          )}
        </div>
      </header>

      <main className="station-content station-content-topnav">{children}</main>

      <span className="station-agent-summary" data-testid="agent-runtime-connection" aria-live="polite">
        {copy(
          `Agent: ${serve.agent_connected ? "Connected" : "Disconnected"}`,
          `Agent：${serve.agent_connected ? "已连接" : "未连接"}`,
        )}
        {copy(
          ` · ${connectedAgents} of ${registry.length} managed`,
          ` · ${connectedAgents} / ${registry.length} 个已接管`,
        )}
        {scanBusy ? copy(" · scanning", " · 扫描中") : ""}
      </span>
    </div>
  );
}
