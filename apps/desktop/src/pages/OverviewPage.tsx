import { useEffect, useState } from "react";
import { Activity, ArrowUpRight, Bot, Boxes, Clock3, Route, WalletCards } from "lucide-react";
import { getStats } from "../api";
import type { AgentUiMetadataView, AgentView, StateView, StatsView, TierSlot } from "../api";
import RevisionChain from "../components/RevisionChain";
import { useLocalizedCopy } from "../components/LanguageProvider";
import { Badge } from "../components/ui/badge";
import { Card, CardContent, CardHeader, CardTitle } from "../components/ui/card";
import { AgentIcon, ProviderIcon } from "../brandIcons";

interface OverviewPageProps {
  state: StateView;
  registry: AgentUiMetadataView[];
  agents: AgentView[];
  onNavigate: (view: "home" | "agents" | "providers" | "usage" | "logs") => void;
}

const TIER_COPY: Record<TierSlot, { en: string; zh: string }> = {
  high: { en: "High", zh: "上档" },
  mid: { en: "Medium", zh: "中档" },
  low: { en: "Low", zh: "下档" },
};

function formatSuccessRate(stats: StatsView) {
  if (stats.total.requests === 0) return null;
  return `${(((stats.total.requests - stats.total.errors) / stats.total.requests) * 100).toFixed(1)}%`;
}

function formatLatency(ms: number) {
  return ms >= 1000 ? `${(ms / 1000).toFixed(1)}s` : `${ms}ms`;
}

function formatCost(costMicros: number | null) {
  if (costMicros == null) return null;
  return new Intl.NumberFormat("en-US", {
    style: "currency",
    currency: "USD",
    minimumFractionDigits: 2,
    maximumFractionDigits: costMicros < 10_000 ? 4 : 2,
  }).format(costMicros / 1_000_000);
}

export default function OverviewPage({ state, registry, agents, onNavigate }: OverviewPageProps) {
  const { copy } = useLocalizedCopy();
  const [stats, setStats] = useState<StatsView | null>(null);
  const [statsUnavailable, setStatsUnavailable] = useState(false);
  const runtimeHealthy = state.serve.app_runtime === "running" && state.serve.listener_reachable;
  const connectedAgents = agents.filter((agent) => agent.status === "CONNECTED").length;
  const agentRows = registry.slice(0, 5).map((metadata) => ({
    metadata,
    agent: agents.find((candidate) => candidate.metadata.agent_id === metadata.agent_id),
  }));
  const modelRows = state.providers.flatMap((provider) => provider.models.map((model) => ({
    model,
    provider,
  }))).slice(0, 5);
  const modelCount = state.providers.reduce((total, provider) => total + provider.models.length, 0);

  useEffect(() => {
    let active = true;
    setStatsUnavailable(false);
    void getStats("24h", null).then((nextStats) => {
      if (active) setStats(nextStats);
    }).catch(() => {
      if (active) setStatsUnavailable(true);
    });
    return () => {
      active = false;
    };
  }, []);

  const successRate = stats ? formatSuccessRate(stats) : null;
  const requestCost = stats ? formatCost(stats.total.cost_micros) : null;
  const statsSummary = statsUnavailable
    ? copy("Statistics are temporarily unavailable", "统计暂不可用")
    : stats == null
      ? copy("Loading local statistics…", "正在读取本地统计…")
      : stats.total.requests === 0
        ? copy("No requests in the last 24 hours", "近 24 小时暂无请求")
        : copy(
            `Success ${successRate} · P95 ${formatLatency(stats.total.p95_latency_ms)}`,
            `成功率 ${successRate} · P95 ${formatLatency(stats.total.p95_latency_ms)}`,
          );

  return (
    <div
      className="page-stack overview-page"
      role="region"
      aria-label={copy("Overview page", "概览页")}
    >
      <header className="overview-heading">
        <div>
          <h1>{copy("Overview", "概览")}</h1>
          <p>{copy(
            "Proxy status, current routing, requests, and cost at a glance.",
            "代理运行状态、当前路由、请求与成本，一屏看清。",
          )}</p>
        </div>
      </header>

      <section className="overview-metrics overview-runtime-metrics" aria-label={copy("System summary", "系统摘要")}>
        <Card size="sm" className="overview-status-card">
          <CardHeader>
            <span><Activity />{copy("Proxy status", "代理状态")}</span>
            <CardTitle><Badge variant={runtimeHealthy ? "default" : "secondary"}><i className={runtimeHealthy ? "healthy" : ""} />{runtimeHealthy ? copy("Running", "运行中") : copy("Stopped", "未运行")}</Badge></CardTitle>
            <dl><div><dt>Revision</dt><dd>{state.saved_revision}</dd></div><div><dt>{copy("Listen", "监听")}</dt><dd>{state.serve.listen}</dd></div></dl>
          </CardHeader>
        </Card>
        <Card size="sm" className="overview-request-card">
          <CardHeader>
            <span><WalletCards />{copy("Cost in the last 24 hours", "近 24 小时成本")}</span>
            <CardTitle className="overview-cost-value">{requestCost ?? (stats ? copy("Cost unpriced", "成本未定价") : "—")}</CardTitle>
            <strong className="overview-request-count"><Clock3 />{stats ? copy(`${stats.total.requests} requests`, `${stats.total.requests} 次请求`) : "—"}</strong>
            <p>{statsSummary}</p>
          </CardHeader>
        </Card>
      </section>

      <section className="overview-summary-grid" aria-label={copy("Workspace summaries", "工作区摘要")}>
        <Card className="overview-summary-card" role="region" aria-label={copy("Agent overview", "Agent 概览")}>
          <CardHeader>
            <span><Bot aria-hidden="true" />Agent</span>
            <CardTitle>{copy(`${registry.length} Agents`, `${registry.length} 个 Agent`)}</CardTitle>
            <p>{copy(`${connectedAgents} managed`, `${connectedAgents} 个已接管`)}</p>
          </CardHeader>
          <CardContent>
            <ul className="overview-summary-list" aria-label={copy("Top Agents", "Agent Top 5")}>
              {agentRows.map(({ metadata, agent }) => (
                <li key={metadata.agent_id}>
                  <AgentIcon id={metadata.agent_id} fallback={metadata.nav_mark ?? metadata.display_name.slice(0, 1)} size={24} />
                  <strong>{metadata.display_name}</strong>
                  <small>{agent?.status === "CONNECTED" ? copy("Managed", "已接管") : copy("Available", "待接入")}</small>
                </li>
              ))}
            </ul>
            <button className="overview-summary-link" type="button" aria-label={copy("Open Agents", "打开 Agent")} onClick={() => onNavigate("agents")}>
              <ArrowUpRight aria-hidden="true" />
            </button>
          </CardContent>
        </Card>

        <Card className="overview-summary-card overview-route-summary" role="region" aria-label={copy("Routing overview", "路由概览")}>
          <CardHeader>
            <span><Route aria-hidden="true" />{copy("Routing", "路由")}</span>
            <CardTitle>{copy("Global routing", "全局路由")}</CardTitle>
            <RevisionChain state={state} />
          </CardHeader>
          <CardContent>
            <div className="overview-route-list">
            {state.routing_mode === "direct" ? (
              <div data-routing-snapshot-mode="direct">
                <Badge variant="outline">{copy("Direct", "单独路由")}</Badge>
                <strong>{state.direct_target?.model ?? copy("Select a model", "待选择模型")}</strong>
                <code>{state.direct_target?.upstream ?? copy("Select a provider", "待选择供应商")}</code>
              </div>
            ) : state.routing_mode === "quota_first" ? (
              <div data-routing-snapshot-mode="quota-first">
                <Badge variant="outline">{copy("Quota-first", "额度优先")}</Badge>
                <strong>{copy(
                  `${state.quota_accounts.length} accounts`,
                  `${state.quota_accounts.length} 个账户`,
                )}</strong>
                <code>{state.quota_accounts.length > 0
                  ? state.quota_accounts
                    .map((account) => `${account.upstream}/${account.model}`)
                    .join(" · ")
                  : copy("Add a quota account", "待添加额度账户")}</code>
              </div>
            ) : (["high", "mid", "low"] as TierSlot[]).map((slot) => {
                const tier = state.tiers[slot];
                return (
                  <div key={slot}>
                    <Badge variant="outline">{copy(TIER_COPY[slot].en, TIER_COPY[slot].zh)}</Badge>
                    <strong>{tier.model ?? copy("Not configured", "未配置")}</strong>
                    <code>{tier.upstream ?? "—"}</code>
                  </div>
                );
              })}
            </div>
            <button className="overview-summary-link" type="button" aria-label={copy("Open routing", "打开路由")} onClick={() => onNavigate("home")}>
              <ArrowUpRight aria-hidden="true" />
            </button>
          </CardContent>
        </Card>

        <Card className="overview-summary-card" role="region" aria-label={copy("Model overview", "模型概览")}>
          <CardHeader>
            <span><Boxes aria-hidden="true" />{copy("Models", "模型")}</span>
            <CardTitle>{copy(`${modelCount} models`, `${modelCount} 个模型`)}</CardTitle>
            <p>{copy(`${state.providers.length} providers`, `${state.providers.length} 个供应商`)}</p>
          </CardHeader>
          <CardContent>
            {modelRows.length > 0 ? (
              <ul className="overview-summary-list overview-model-list" aria-label={copy("Top models", "模型 Top 5")}>
                {modelRows.map(({ model, provider }) => (
                  <li key={`${provider.name}:${model}`}>
                    <ProviderIcon id={provider.brand_id} label={provider.name} size={24} />
                    <strong>{model}</strong>
                    <small>{provider.name}</small>
                  </li>
                ))}
              </ul>
            ) : (
              <p className="overview-summary-empty">{copy("No models are connected.", "尚未接入模型。")}</p>
            )}
            <button className="overview-summary-link" type="button" aria-label={copy("Open models", "打开模型")} onClick={() => onNavigate("providers")}>
              <ArrowUpRight aria-hidden="true" />
            </button>
          </CardContent>
        </Card>
      </section>
    </div>
  );
}
