import { useEffect, useState } from "react";
import { Activity, ArrowUpRight, Bot, Boxes, CircleCheckBig, Clock3, DollarSign, MessageSquareText, Route, Zap } from "lucide-react";
import { getStats } from "../api";
import type { AgentUiMetadataView, AgentView, StateView, StatsView, TierSlot } from "../api";
import { useLocalizedCopy } from "../components/LanguageProvider";
import UsageTrendChart from "../components/UsageTrendChart";
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
  return costMicros == null ? "—" : (costMicros / 1_000_000).toFixed(4);
}

export default function OverviewPage({ state, registry, agents, onNavigate }: OverviewPageProps) {
  const { copy, language } = useLocalizedCopy();
  const [stats, setStats] = useState<StatsView | null>(null);
  const [trend, setTrend] = useState<StatsView | null>(null);
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
        label: copy(`Global · ${routeModeName(state.routing_mode)}`, `全局 · ${routeModeName(state.routing_mode)}`, `全域性 · ${routeModeName(state.routing_mode)}`, `グローバル · ${routeModeName(state.routing_mode)}`),
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
    void Promise.all([
      getStats("24h", null),
      getStats("24h", "hour"),
    ]).then(([nextStats, nextTrend]) => {
      if (!active) return;
      setStats(nextStats);
      setTrend(nextTrend);
    }).catch(() => {
      if (active) setStatsUnavailable(true);
    });
    return () => {
      active = false;
    };
  }, []);

  const successRate = stats ? formatSuccessRate(stats) : null;
  const totalTokens = stats ? stats.total.input_tokens + stats.total.output_tokens : null;

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

      <section
        className="usage-trend-panel overview-usage-trend"
        role="region"
        aria-label={copy("Usage trend for the last 24 hours", "近 24 小时使用趋势", "近 24 小時使用趨勢", "過去24時間の使用状況の推移")}
      >
        <header>
          <div>
            <div>
              <h2>{copy("Usage trend", "使用趋势", "使用趨勢", "使用トレンド")}</h2>
              <p>{copy("Last 24 hours · Hourly", "近 24 小时 · 按小时", "近 24 小時 · 按小時", "過去24時間 · 1時間ごと")}</p>
            </div>
          </div>
          <div className="overview-usage-trend-actions">
            <span className="usage-dual-axis-note">{copy("Left: tokens · Right: cost", "左轴 Token · 右轴成本", "左軸 Token · 右軸成本", "左軸 Token · 右軸コスト")}</span>
            <button
              className="overview-usage-open"
              type="button"
              aria-label={copy("Open full Usage", "打开完整用量", "開啟完整用量", "使用量の詳細を開く")}
              onClick={() => onNavigate("usage")}
            >
              <ArrowUpRight aria-hidden="true" />
            </button>
          </div>
        </header>
        <div className="usage-chart-legend">
          <span><i className="tone-input" />{copy("Input total", "输入总量", "輸入總量", "入力合計")}</span>
          <span><i className="tone-output" />{copy("Output", "输出", "輸出", "出力")}</span>
          <span><i className="tone-cache-write" />{copy("Cache write", "缓存写入", "快取寫入", "キャッシュ書き込み")}</span>
          <span><i className="tone-cache-read" />{copy("Cache hit", "缓存命中", "快取命中", "キャッシュヒット")}</span>
          <span><i className="tone-cost" />{copy("Cost", "成本", "成本", "コスト")}</span>
        </div>
        {statsUnavailable ? (
          <div className="overview-usage-error">{copy("Statistics are temporarily unavailable", "统计暂不可用", "統計暫不可用", "統計は一時的に利用不可です")}</div>
        ) : trend == null ? (
          <div className="overview-usage-loading">{copy("Loading local statistics…", "正在读取本地统计…", "正在讀取本地統計…", "ローカルの統計を読み込んでいます…")}</div>
        ) : (
          <UsageTrendChart groups={trend?.groups ?? []} range="24h" />
        )}
      </section>

      <section
        className="overview-usage-metrics"
        role="region"
        aria-label={copy("Usage metrics for the last 24 hours", "近 24 小时用量指标", "近 24 小時用量指標", "過去24時間の使用量指標")}
      >
        <article className="overview-usage-metric primary">
          <span className="overview-usage-metric-icon"><Zap aria-hidden="true" /></span>
          <div>
            <span>{copy("Total tokens", "总 Token", "總 Token", "合計トークン")}</span>
            <strong>{totalTokens?.toLocaleString(language) ?? "—"}</strong>
            <small>{copy("input + output", "输入 + 输出", "輸入 + 輸出", "入力 + 出力")}</small>
          </div>
        </article>
        <article className="overview-usage-metric request">
          <Activity aria-hidden="true" />
          <span>{copy("Requests", "请求", "請求", "リクエスト")}</span>
          <strong>{stats?.total.requests.toLocaleString(language) ?? "—"}</strong>
        </article>
        <article className="overview-usage-metric cost">
          <DollarSign aria-hidden="true" />
          <span>{copy("Estimated cost", "估算成本", "估算成本", "推定コスト")}</span>
          <strong>{stats ? formatCost(stats.total.cost_micros) : "—"}</strong>
        </article>
        <article className="overview-usage-metric success">
          <CircleCheckBig aria-hidden="true" />
          <span>{copy("Success rate", "成功率", "成功率", "成功率")}</span>
          <strong>{successRate ?? "—"}</strong>
        </article>
        <article className="overview-usage-metric latency">
          <Clock3 aria-hidden="true" />
          <span>{copy("p95 latency", "p95 延迟", "p95 延遲", "p95 レイテンシー")}</span>
          <strong>{stats ? formatLatency(stats.total.p95_latency_ms) : "—"}</strong>
        </article>
      </section>

      <section className="overview-metrics overview-runtime-metrics" aria-label={copy("System summary", "系统摘要", "系統摘要", "システムサマリー")}>
        <Card size="sm" className="overview-status-card">
          <CardHeader>
            <span><Activity />{copy("Proxy status", "代理状态", "代理狀態", "プロキシステータス")}</span>
            <CardTitle><Badge variant={runtimeHealthy ? "default" : "secondary"}><i className={runtimeHealthy ? "healthy" : ""} />{runtimeHealthy ? copy("Running", "运行中", "執行中", "実行中") : copy("Stopped", "未运行", "已停止", "停止中")}</Badge></CardTitle>
            <dl><div><dt>{copy("Revision", "版本", "版本", "リビジョン")}</dt><dd>{state.saved_revision}</dd></div><div><dt>{copy("Listen", "监听", "監聽", "リスニング")}</dt><dd>{state.serve.listen}</dd></div></dl>
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
                  <Badge variant="outline">{copy("Direct", "简单路由", "簡單路由", "シンプルルーティング")}</Badge>
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
            <button className="overview-summary-link" type="button" aria-label={copy("Open routing", "打开路由", "開啟路由", "ルーティングを開く")} onClick={() => onNavigate("home")}>
              <ArrowUpRight aria-hidden="true" />
            </button>
          </CardContent>
        </Card>

        <Card className="overview-summary-card" role="region" aria-label={copy("Model overview", "模型概览", "模型概覽", "モデル概要")}>
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
