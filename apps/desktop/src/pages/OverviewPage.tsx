import { useEffect, useState } from "react";
import { Activity, AlertTriangle, Bot, Boxes, Clock3, Route } from "lucide-react";
import { getStats } from "../api";
import type { AgentUiMetadataView, AgentView, StateView, StatsView, TierSlot } from "../api";
import RevisionChain from "../components/RevisionChain";
import { useLocalizedCopy } from "../components/LanguageProvider";
import { Badge } from "../components/ui/badge";
import { Button } from "../components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "../components/ui/card";
import { Separator } from "../components/ui/separator";

interface OverviewPageProps {
  state: StateView;
  registry: AgentUiMetadataView[];
  agents: AgentView[];
  onNavigate: (view: "home" | "agents" | "providers" | "usage" | "logs") => void;
  onRescan: () => void;
  scanBusy: boolean;
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

export default function OverviewPage({ state, registry, agents, onNavigate, onRescan, scanBusy }: OverviewPageProps) {
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
    <div className="page-stack overview-page">
      <header className="overview-heading">
        <div>
          <span className="page-eyebrow">CONTROL PLANE</span>
          <h1>{copy("System overview", "系统概览")}</h1>
          <p>{copy(
            "Runtime, routing revision, and the next actions that need attention.",
            "代理运行状态、路由版本和需要处理的异常，一屏掌握。",
          )}</p>
        </div>
        <div className="overview-heading-actions">
          <Button variant="outline" size="sm" onClick={onRescan} disabled={scanBusy}>
            <Bot />{scanBusy ? copy("Scanning…", "扫描中…") : copy("Rescan Agents", "重新扫描 Agent")}
          </Button>
          <Button size="sm" onClick={() => onNavigate("providers")}>
            <Boxes />{copy("Manage providers", "管理供应商")}
          </Button>
        </div>
      </header>

      <Card className="overview-runtime-card">
        <CardContent className="overview-runtime-content">
          <div className="overview-runtime-identity">
            <span><Activity /></span>
            <div>
              <small>{copy("Local proxy", "本地代理")}</small>
              <strong><i className={runtimeHealthy ? "healthy" : ""} />{runtimeHealthy ? copy("Running normally", "运行正常") : copy("Not running", "未运行")}</strong>
            </div>
          </div>
          <Separator orientation="vertical" />
          <RevisionChain state={state} />
          <Separator orientation="vertical" />
          <dl>
            <div><dt>{copy("Listen", "监听地址")}</dt><dd>{state.serve.listen}</dd></div>
            <div><dt>{copy("Agent runtime", "Agent 连接")}</dt><dd>{state.serve.agent_connected ? copy("Connected", "已连接") : copy("Disconnected", "未连接")}</dd></div>
          </dl>
        </CardContent>
      </Card>

      <section className="overview-metrics" aria-label={copy("System summary", "系统摘要")}>
        <Card size="sm"><CardHeader><span><Bot />{copy("Managed Agents", "已接管 Agent")}</span><CardTitle>{connectedAgents} <small>/ {registry.length}</small></CardTitle><p>{copy(`${pendingAgents} detected and pending`, `${pendingAgents} 个已检测待接入`)}</p></CardHeader></Card>
        <Card size="sm"><CardHeader><span><Boxes />{copy("Providers", "供应商")}</span><CardTitle>{state.providers.length}</CardTitle><p>{copy("Configured upstreams", "已配置上游")}</p></CardHeader></Card>
        <Card size="sm"><CardHeader><span><Clock3 />{copy("Requests · 24h", "近 24 小时请求")}</span><CardTitle>{stats?.total.requests ?? "—"}</CardTitle><p>{statsSummary}</p></CardHeader></Card>
        <Card size="sm" className={pendingAgents > 0 ? "attention" : ""}><CardHeader><span><AlertTriangle />{copy("Needs attention", "待处理")}</span><CardTitle>{pendingAgents}</CardTitle><p>{copy("Detected Agents not managed", "已检测但未接管的 Agent")}</p></CardHeader></Card>
      </section>

      <section className="overview-main-grid">
        <Card>
          <CardHeader><CardTitle>{copy("Current routing snapshot", "当前路由快照")}</CardTitle><Badge variant="secondary">rev {state.saved_revision}</Badge></CardHeader>
          <CardContent className="overview-route-list">
            {(["high", "mid", "low"] as TierSlot[]).map((slot) => {
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
          <CardHeader><CardTitle>{copy("Next actions", "下一步处理")}</CardTitle></CardHeader>
          <CardContent className="overview-actions-list">
            <button type="button" onClick={() => onNavigate("agents")}><Bot /><span><strong>{copy("Review Agent connections", "检查 Agent 接入")}</strong><small>{copy(`${pendingAgents} detected Agents are not managed`, `${pendingAgents} 个已检测 Agent 尚未接管`)}</small></span></button>
            <button type="button" onClick={() => onNavigate("usage")}><Clock3 /><span><strong>{copy("Review local usage", "查看本地用量")}</strong><small>{copy("Requests, reliability, tokens, and cost", "请求、成功率、Token 与成本")}</small></span></button>
            <button type="button" onClick={() => onNavigate("logs")}><Activity /><span><strong>{copy("Inspect request receipts", "检查请求回执")}</strong><small>{copy("Routing decisions without prompt or response bodies", "仅含路由决策，不含提示词与响应正文")}</small></span></button>
          </CardContent>
        </Card>
      </section>
    </div>
  );
}
