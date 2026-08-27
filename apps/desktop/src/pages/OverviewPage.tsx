import { useEffect, useState } from "react";
import { Activity, ArrowUpRight, Bot, Boxes, Clock3, MessageSquareText, Route, WalletCards } from "lucide-react";
import { getStats } from "../api";
import type { AgentUiMetadataView, AgentView, StateView, StatsView, TierSlot } from "../api";
import { useLocalizedCopy } from "../components/LanguageProvider";
import { Badge } from "../components/ui/badge";
import { Card, CardContent, CardHeader, CardTitle } from "../components/ui/card";
import { AgentIcon, ProviderIcon } from "../brandIcons";
import ModelTestConsole from "../components/ModelTestConsole";
import { Button } from "../components/ui/button";

interface OverviewPageProps {
  state: StateView;
  registry: AgentUiMetadataView[];
  agents: AgentView[];
  onNavigate: (view: "home" | "agents" | "providers" | "usage" | "logs") => void;
}

const TIER_COPY: Record<TierSlot, [string, string, string, string]> = {
  high: ["High", "上档", "高階", "高"],
  mid: ["Medium", "中档", "中階", "中"],
  low: ["Low", "下档", "低階", "低"],
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
  const [modelTestOpen, setModelTestOpen] = useState(false);
  const runtimeHealthy = state.serve.app_runtime === "running" && state.serve.listener_reachable;
  const connectedAgentIds = new Set(
    agents
      .filter((agent) => agent.status === "CONNECTED")
      .map((agent) => agent.metadata.agent_id),
  );
  const connectedAgents = connectedAgentIds.size;
  const orderedAgentMetadata = [
    ...registry.filter((metadata) => connectedAgentIds.has(metadata.agent_id)),
    ...registry.filter((metadata) => !connectedAgentIds.has(metadata.agent_id)),
  ];
  const agentRows = orderedAgentMetadata.slice(0, 5).map((metadata) => ({
    metadata,
    agent: agents.find((candidate) => candidate.metadata.agent_id === metadata.agent_id),
  }));
  const connectedRouteAgents = agents
    .filter((agent) => agent.status === "CONNECTED")
    .sort((left, right) => {
      const leftIndex = registry.findIndex((metadata) => metadata.agent_id === left.metadata.agent_id);
      const rightIndex = registry.findIndex((metadata) => metadata.agent_id === right.metadata.agent_id);
      return (leftIndex < 0 ? Number.MAX_SAFE_INTEGER : leftIndex)
        - (rightIndex < 0 ? Number.MAX_SAFE_INTEGER : rightIndex);
    })
    .slice(0, 5);
  const modelRows = state.providers.flatMap((provider) => provider.models.map((model) => ({
    model,
    provider,
  }))).slice(0, 5);
  const modelCount = state.providers.reduce((total, provider) => total + provider.models.length, 0);
  const activeEnterpriseProvider = state.routing_mode === "direct"
    ? state.providers.find((provider) => (
      provider.name === state.direct_target?.upstream && provider.managed_route
    )) ?? null
    : null;

  const routeModeName = (mode: StateView["routing_mode"]) => {
    if (mode === "direct") return copy("Direct", "简单路由", "簡單路由", "シンプルルーティング");
    if (mode === "quota_first") return copy("Quota-first", "额度优先", "額度優先", "クォータ優先");
    return copy("Smart routing", "智能路由", "智慧路由", "スマートルーティング");
  };
  const routeRows = connectedRouteAgents.map(({ metadata }) => {
    const route = state.agent_routes?.[metadata.agent_id];
    const inherited = !route || route.mode === "inherit";
    if (inherited) {
      return {
        metadata,
        label: activeEnterpriseProvider
          ? copy("Enterprise · Managed route", "企业 · 托管路由", "企業 · 託管路由", "企業 · 管理ルート")
          : copy(`Global · ${routeModeName(state.routing_mode)}`, `全局 · ${routeModeName(state.routing_mode)}`, `全域性 · ${routeModeName(state.routing_mode)}`, `グローバル · ${routeModeName(state.routing_mode)}`),
      };
    }
    if (route.mode === "profile") {
      return {
        metadata,
        label: copy("Profile", "策略组", "策略群組", "プロファイル"),
      };
    }
    return {
      metadata,
      label: copy(`Custom · ${routeModeName(route.routing_mode)}`, `独立 · ${routeModeName(route.routing_mode)}`, `獨立 · ${routeModeName(route.routing_mode)}`, `独立 · ${routeModeName(route.routing_mode)}`),
    };
  });

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
    ? copy("Statistics are temporarily unavailable", "统计暂不可用", "統計暫不可用", "統計は一時的に利用不可です")
    : stats == null
      ? copy("Loading local statistics…", "正在读取本地统计…", "正在讀取本地統計…", "ローカルの統計を読み込んでいます…")
      : stats.total.requests === 0
        ? copy("No requests in the last 24 hours", "近 24 小时暂无请求", "近 24 小時暫無請求", "過去24時間のリクエストはまだありません")
        : copy(
            `Success ${successRate} · P95 ${formatLatency(stats.total.p95_latency_ms)}`,
            `成功率 ${successRate} · P95 ${formatLatency(stats.total.p95_latency_ms)}`, `成功率 ${successRate} · P95 ${formatLatency(stats.total.p95_latency_ms)}`, `成功率 ${successRate} · P95 ${formatLatency(stats.total.p95_latency_ms)}`
          );

  return (
    <div
      className="page-stack overview-page"
      role="region"
      aria-label={copy("Overview page", "概览页", "概覽頁", "概要ページ")}
    >
      <header className="overview-heading">
        <div>
          <h1>{copy("Overview", "概览", "概覽", "概要")}</h1>
          <p>{copy(
            "Proxy status, current routing, requests, and cost at a glance.",
            "代理运行状态、当前路由、请求与成本，一屏看清。", "代理執行狀態、當前路由、請求與成本，一屏看清。", "プロキシのステータス、現在のルーティング、リクエストとコストを一画面で確認できます。"
          )}</p>
        </div>
      </header>

      <section className="overview-metrics overview-runtime-metrics" aria-label={copy("System summary", "系统摘要", "系統摘要", "システムサマリー")}>
        <Card size="sm" className="overview-status-card">
          <CardHeader>
            <span><Activity />{copy("Proxy status", "代理状态", "代理狀態", "プロキシステータス")}</span>
            <CardTitle><Badge variant={runtimeHealthy ? "default" : "secondary"}><i className={runtimeHealthy ? "healthy" : ""} />{runtimeHealthy ? copy("Running", "运行中", "執行中", "実行中") : copy("Stopped", "未运行", "已停止", "停止中")}</Badge></CardTitle>
            <dl><div><dt>{copy("Revision", "版本", "版本", "リビジョン")}</dt><dd>{state.saved_revision}</dd></div><div><dt>{copy("Listen", "监听", "監聽", "リスニング")}</dt><dd>{state.serve.listen}</dd></div></dl>
          </CardHeader>
        </Card>
        <Card size="sm" className="overview-request-card">
          <CardHeader>
            <span><WalletCards />{copy("Cost in the last 24 hours", "近 24 小时成本", "近 24 小時成本", "過去 24 時間のコスト")}</span>
            <CardTitle className="overview-cost-value">{requestCost ?? (stats ? copy("Cost unpriced", "成本未定价", "成本未定價", "コストが未設定") : "—")}</CardTitle>
            <strong className="overview-request-count"><Clock3 />{stats ? copy(`${stats.total.requests} requests`, `${stats.total.requests} 次请求`, `${stats.total.requests} 次請求`, `${stats.total.requests} 回のリクエスト`) : "—"}</strong>
            <p>{statsSummary}</p>
          </CardHeader>
        </Card>
      </section>

      <section className="overview-summary-grid" aria-label={copy("Workspace summaries", "工作区摘要", "工作區摘要", "ワークスペースサマリー")}>
        <Card className="overview-summary-card overview-agent-summary" role="region" aria-label={copy("Agent overview", "Agent 概览", "Agent 總覽", "Agentの概要")}>
          <CardHeader>
            <span><Bot aria-hidden="true" />Agent</span>
            <CardTitle>{copy(
              `${connectedAgents} ${connectedAgents === 1 ? "Agent" : "Agents"} connected`,
              `已接入 ${connectedAgents} 个 Agent`,
              `已連線 ${connectedAgents} 個 Agent`,
              `${connectedAgents} 個の Agent が接続済み`,
            )}</CardTitle>
          </CardHeader>
          <CardContent>
            <ul className="overview-summary-list" aria-label={copy("Top Agents", "Agent Top 5", "前 5 個 Agent", "Agent トップ5")}>
              {agentRows.map(({ metadata, agent }) => (
                <li key={metadata.agent_id}>
                  <AgentIcon id={metadata.agent_id} fallback={metadata.nav_mark ?? metadata.display_name.slice(0, 1)} size={24} />
                  <strong>{metadata.display_name}</strong>
                  <small>{agent?.status === "CONNECTED" ? copy("Managed", "已接管", "已接管", "管理中") : copy("Available", "待接入", "待接入", "接続待ち")}</small>
                </li>
              ))}
            </ul>
            <div className="overview-agent-actions">
              <Button
                type="button"
                variant="outline"
                size="sm"
                className="overview-model-test-action"
                disabled={modelCount === 0}
                onClick={() => setModelTestOpen(true)}
              >
                <MessageSquareText aria-hidden="true" />
                {copy("Verify model connection", "验证模型连接", "驗證模型連線", "モデル接続を確認")}
              </Button>
              <button className="overview-summary-link" type="button" aria-label={copy("Open Agents", "打开 Agent", "開啟 Agent", "Agentを開く")} onClick={() => onNavigate("agents")}>
                <ArrowUpRight aria-hidden="true" />
              </button>
            </div>
          </CardContent>
        </Card>

        <Card className="overview-summary-card overview-route-summary" role="region" aria-label={copy("Routing overview", "路由概览", "路由總覽", "ルーティングの概要")}>
          <CardHeader>
            <span><Route aria-hidden="true" />{copy("Routing", "路由", "路由", "ルーティング")}</span>
            <CardTitle>{routeRows.length > 0
              ? copy("Agent routing", "Agent 路由", "Agent 路由", "Agent ルーティング")
              : activeEnterpriseProvider
                ? copy("Enterprise routing", "企业路由", "企業路由", "企業ルーティング")
                : copy("Global routing", "全局路由", "全域路由", "グローバルルーティング")}</CardTitle>
          </CardHeader>
          <CardContent>
            {routeRows.length > 0 ? (
              <ul className="overview-summary-list overview-agent-route-list" aria-label={copy("Agent route Top 5", "Agent 路由 Top 5", "前 5 個 Agent 路由", "Agent ルーティング上位5件")}>
                {routeRows.map(({ metadata, label }) => (
                  <li key={metadata.agent_id}>
                    <AgentIcon id={metadata.agent_id} fallback={metadata.nav_mark ?? metadata.display_name.slice(0, 1)} size={24} />
                    <strong>{metadata.display_name}</strong>
                    <span className="overview-agent-route-target" title={label}>
                      <small>{label}</small>
                    </span>
                  </li>
                ))}
              </ul>
            ) : state.routing_mode === "direct" ? (
              <div className="overview-route-list overview-global-route-list" data-routing-snapshot-mode="direct">
                <div>
                  <Badge variant="outline">{activeEnterpriseProvider
                    ? copy("Managed route", "托管路由", "託管路由", "管理ルート")
                    : copy("Direct", "简单路由", "簡單路由", "シンプルルーティング")}</Badge>
                  <strong>{state.direct_target?.model ?? copy("Select a model", "待选择模型", "待選擇模型", "選択するモデル")}</strong>
                  <code>{state.direct_target?.upstream ?? copy("Select a provider", "待选择供应商", "選擇供應商", "プロバイダーを選択")}</code>
                </div>
              </div>
            ) : state.routing_mode === "quota_first" ? (
              <div className="overview-route-list overview-global-route-list" data-routing-snapshot-mode="quota-first">
                <div>
                  <Badge variant="outline">{copy("Quota-first", "额度优先", "額度優先", "クォータ優先")}</Badge>
                  <strong>{copy(
                    `${state.quota_accounts.length} accounts`,
                    `${state.quota_accounts.length} 个账户`, `${state.quota_accounts.length} 個帳號`, `${state.quota_accounts.length} 個のアカウント`
                  )}</strong>
                  <code>{state.quota_accounts.length > 0
                    ? state.quota_accounts
                      .map((account) => `${account.upstream}/${account.model}`)
                      .join(" · ")
                    : copy("Add a quota account", "待添加额度账户", "新增額度帳號", "クォータアカウントを追加")}</code>
                </div>
              </div>
            ) : (
              <div className="overview-route-list overview-global-route-list" data-routing-snapshot-mode="tiered">
                {(["high", "mid", "low"] as TierSlot[]).map((slot) => {
                  const tier = state.tiers[slot];
                  return (
                    <div key={slot}>
                      <Badge variant="outline">{copy(...TIER_COPY[slot])}</Badge>
                      <strong>{tier.model ?? copy("Not configured", "未配置", "未配置", "未設定")}</strong>
                      <code>{tier.upstream ?? "—"}</code>
                    </div>
                  );
                })}
              </div>
            )}
            <button className="overview-summary-link" type="button" aria-label={copy("Open routing", "打开路由", "開啟路由", "ルーティングを開く")} onClick={() => onNavigate(activeEnterpriseProvider ? "providers" : "home")}>
              <ArrowUpRight aria-hidden="true" />
            </button>
          </CardContent>
        </Card>

        <Card className="overview-summary-card overview-model-summary" role="region" aria-label={copy("Model overview", "模型概览", "模型概覽", "モデル概要")}>
          <CardHeader>
            <span><Boxes aria-hidden="true" />{copy("Models", "模型", "模型", "モデル")}</span>
            <CardTitle>{copy(`${modelCount} models`, `${modelCount} 个模型`, `${modelCount} 個模型`, `${modelCount} 個モデル`)}</CardTitle>
            <p>{copy(`${state.providers.length} providers`, `${state.providers.length} 个供应商`, `${state.providers.length} 個供應商`, `${state.providers.length} 個のプロバイダー`)}</p>
          </CardHeader>
          <CardContent>
            {modelRows.length > 0 ? (
              <ul className="overview-summary-list overview-model-list" aria-label={copy("Top models", "模型 Top 5", "前 5 個模型", "上位5件のモデル")}>
                {modelRows.map(({ model, provider }) => (
                  <li key={`${provider.name}:${model}`}>
                    <ProviderIcon id={provider.brand_id} label={provider.name} size={24} />
                    <strong>{model}</strong>
                    <small>{provider.name}</small>
                  </li>
                ))}
              </ul>
            ) : (
              <p className="overview-summary-empty">{copy("No models are connected.", "尚未接入模型。", "尚未接入模型。", "モデルが接続されていません。")}</p>
            )}
            <button className="overview-summary-link" type="button" aria-label={copy("Open models", "打开模型", "開啟模型", "モデルを開く")} onClick={() => onNavigate("providers")}>
              <ArrowUpRight aria-hidden="true" />
            </button>
          </CardContent>
        </Card>
      </section>
      <ModelTestConsole
        open={modelTestOpen}
        onOpenChange={setModelTestOpen}
        routingMode={state.routing_mode}
        routeState={state.serve.model_test_uses_running_gateway ? "running" : "draft"}
      />
    </div>
  );
}
