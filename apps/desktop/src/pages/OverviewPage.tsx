import { useEffect, useState } from "react";
import { Activity, Bot, Boxes, Clock3, Route, WalletCards } from "lucide-react";
import { getStats } from "../api";
import type { AgentUiMetadataView, AgentView, StateView, StatsView, TierSlot } from "../api";
import RevisionChain from "../components/RevisionChain";
import { useLocalizedCopy } from "../components/LanguageProvider";
import { Badge } from "../components/ui/badge";
import { Button } from "../components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "../components/ui/card";

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

function agentHasInstall(agent: AgentView) {
  return agent.installations.length > 0;
}

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
  const detectedAgents = agents.filter(agentHasInstall).length;
  const pendingAgents = Math.max(0, detectedAgents - connectedAgents);

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

      <section
        className="overview-metrics"
        aria-label={copy("System summary", "系统摘要")}
      >
        <Card size="sm" className="overview-status-card">
          <CardHeader>
            <span><Activity />{copy("Proxy status", "代理状态")}</span>
            <CardTitle><Badge variant={runtimeHealthy ? "default" : "secondary"}><i className={runtimeHealthy ? "healthy" : ""} />{runtimeHealthy ? copy("Running", "运行中") : copy("Stopped", "未运行")}</Badge></CardTitle>
            <dl><div><dt>Revision</dt><dd>{state.saved_revision}</dd></div><div><dt>{copy("Listen", "监听")}</dt><dd>{state.serve.listen}</dd></div></dl>
          </CardHeader>
        </Card>
        <Card size="sm" className="overview-request-card">
          <CardHeader>
            <span><WalletCards />{copy("Cost today", "今日成本")}</span>
            <CardTitle className="overview-cost-value">{requestCost ?? (stats ? copy("Cost unpriced", "成本未定价") : "—")}</CardTitle>
            <strong className="overview-request-count"><Clock3 />{stats ? copy(`${stats.total.requests} requests`, `${stats.total.requests} 次请求`) : "—"}</strong>
            <p>{statsSummary} · {copy("rolling 24h", "近 24 小时口径")}</p>
          </CardHeader>
        </Card>
        <Card size="sm"><CardHeader><span><Bot />{copy("Managed Agents", "已接管 Agent")}</span><CardTitle>{connectedAgents} <small>/ {registry.length}</small></CardTitle><p>{copy(`${pendingAgents} detected and pending`, `${pendingAgents} 个已检测待接入`)}</p></CardHeader></Card>
        <Card size="sm" className={state.quota_accounts.length > 0 ? "attention" : ""}><CardHeader><span><Boxes />{copy("Providers", "供应商")}</span><CardTitle>{state.providers.length}</CardTitle><p>{copy(`${state.quota_accounts.length} quota accounts configured`, `${state.quota_accounts.length} 个额度账户已配置`)}</p></CardHeader></Card>
      </section>

      <section className="overview-main-grid">
        <Card role="region" aria-label={copy("Current routing snapshot", "当前路由快照")}>
          <CardHeader><CardTitle>{copy("Current routing snapshot", "当前路由快照")}</CardTitle><RevisionChain state={state} /></CardHeader>
          <CardContent className="overview-route-list">
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
            <Button variant="ghost" size="sm" onClick={() => onNavigate("home")}><Route />{copy("Adjust global routing", "调整全局路由")}</Button>
          </CardContent>
        </Card>
        <Card>
          <CardHeader><CardTitle>{copy("Shortcuts", "快捷键")}</CardTitle></CardHeader>
          <CardContent className="overview-actions-list">
            <button data-onboarding-target="agent-connect" type="button" onClick={() => onNavigate("agents")}><Bot /><span><strong>{copy("Review Agent connections", "检查 Agent 接入")}</strong><small>{copy(`${pendingAgents} detected Agents are not managed`, `${pendingAgents} 个已检测 Agent 尚未接管`)}</small></span></button>
            <button type="button" onClick={() => onNavigate("usage")}><Clock3 /><span><strong>{copy("Review local usage", "查看本地用量")}</strong><small>{copy("Requests, reliability, tokens, and cost", "请求、成功率、Token 与成本")}</small></span></button>
            <button type="button" onClick={() => onNavigate("logs")}><Activity /><span><strong>{copy("Inspect request receipts", "检查请求回执")}</strong><small>{copy("Routing decisions without prompt or response bodies", "仅含路由决策，不含提示词与响应正文")}</small></span></button>
          </CardContent>
        </Card>
      </section>
    </div>
  );
}
