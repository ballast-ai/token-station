import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  AgentUiMetadataView,
  AggView,
  BudgetStatus,
  StatsView,
  getAgentBudgets,
  getStats,
  listAgentRegistry,
} from "../api";
import PageBackButton from "../components/PageBackButton";
import CompactCombobox from "../components/CompactCombobox";
import UsageTrendChart, { type UsageTrendRange } from "../components/UsageTrendChart";
import { useLocalizedCopy, type LocalizedCopy } from "../components/LanguageProvider";
import { humanizeAppError } from "../errors";
import { useErrorToast } from "../components/ErrorToast";
import { SlidersHorizontal } from "lucide-react";

export function formatBudgetAmount(micros: number): string {
  if (micros === 0) return "0.00";
  const amount = micros / 1_000_000;
  if (Math.abs(amount) >= 0.01) return amount.toFixed(2);
  return amount.toFixed(6).replace(/0+$/, "").replace(/\.$/, "");
}

type GroupValue = "agent" | "upstream" | "model" | "status" | "engine" | "fallback";

const EMPTY_AGG: AggView = {
  requests: 0,
  errors: 0,
  p50_latency_ms: 0,
  p95_latency_ms: 0,
  input_tokens: 0,
  output_tokens: 0,
  cache_read_tokens: 0,
  cache_write_tokens: 0,
  reasoning_tokens: 0,
  cost_micros: null,
  priced_requests: 0,
  unpriced_requests: 0,
};

function compact(value: number, locale: string, digits = 1): string {
  return new Intl.NumberFormat(locale, {
    notation: "compact",
    maximumFractionDigits: digits,
  }).format(value);
}

function cost(micros: number | null): string {
  return micros == null ? "—" : (micros / 1_000_000).toFixed(4);
}

function successRate(aggregate: AggView): string {
  if (!aggregate.requests) return "—";
  return `${(((aggregate.requests - aggregate.errors) / aggregate.requests) * 100).toFixed(1)}%`;
}

function cacheRate(aggregate: AggView): string {
  if (!aggregate.input_tokens) return "—";
  return `${((aggregate.cache_read_tokens / aggregate.input_tokens) * 100).toFixed(1)}%`;
}

function latency(ms: number): string {
  if (ms >= 1000) return `${(ms / 1000).toFixed(ms >= 10_000 ? 0 : 1)}s`;
  return `${ms}ms`;
}

function budgetWarning(
  status: BudgetStatus,
  name: string,
  copy: LocalizedCopy,
): string {
  const parts: string[] = [];
  if (status.usage_level === "approaching") {
    parts.push(copy(
      `${name} has used ${status.usage_percent.toFixed(1)}% and is approaching the budget limit`,
      `${name} 已使用 ${status.usage_percent.toFixed(1)}%，接近预算上限`, `${name} 已使用 ${status.usage_percent.toFixed(1)}%，接近預算上限`, `${name} は ${status.usage_percent.toFixed(1)}% 使用しており、予算上限に近づいています`
    ));
  } else if (status.usage_level === "exceeded") {
    parts.push(copy(
      `${name} has used ${status.usage_percent.toFixed(1)}% and exceeded the budget limit`,
      `${name} 已使用 ${status.usage_percent.toFixed(1)}%，已超过预算上限`, `${name} 已使用 ${status.usage_percent.toFixed(1)}%，已超過預算上限`, `${name} は ${status.usage_percent.toFixed(1)}% 使用しており、予算上限を超過しています`
    ));
  } else if (status.usage_level === "unknown") {
    parts.push(copy(
      `${name} has ${status.unpriced_requests} unpriced requests, so budget usage is incomplete`,
      `${name} 有 ${status.unpriced_requests} 个请求未定价，预算用量不完整`, `${name} 有 ${status.unpriced_requests} 個未計價請求，預算用量不完整`, `${name} には ${status.unpriced_requests} 個の未価格リクエストがあり、予算使用量が不完全です`
    ));
  } else {
    parts.push(copy(
      `${name} has used ${status.usage_percent.toFixed(1)}%`,
      `${name} 已使用 ${status.usage_percent.toFixed(1)}%`, `${name} 已使用 ${status.usage_percent.toFixed(1)}%`, `${name} は ${status.usage_percent.toFixed(1)}% 使用しています`
    ));
  }
  if (status.expiry_level === "expiring") parts.push(copy("Budget period expires soon", "预算周期即将到期", "預算週期即將到期", "予算期間が間もなく終了します"));
  if (status.expiry_level === "expired") parts.push(copy("Budget period expired", "预算周期已到期", "預算週期已到期", "予算期間がすでに終了しています"));
  return parts.join(" · ");
}

function ToneIcon({ type }: { type: "token" | "request" | "cost" | "success" | "latency" }) {
  const paths = {
    token: <path d="M11 2 4.5 11H10l-1 7 6.5-9H10l1-7Z" />,
    request: <><path d="M3 10h4l2-6 3 12 2-6h4" /><path d="M3 18h15" /></>,
    cost: <><circle cx="11" cy="11" r="8" /><path d="M13.8 7.7c-.7-.6-1.6-.9-2.7-.9-1.5 0-2.5.7-2.5 1.8 0 2.8 5.3 1.2 5.3 4 0 1.1-1 2-2.7 2-1.2 0-2.3-.4-3.1-1.1M11 5.2v11.6" /></>,
    success: <><circle cx="11" cy="11" r="8" /><path d="m7.5 11 2.2 2.2 4.8-5" /></>,
    latency: <><circle cx="11" cy="11" r="8" /><path d="M11 6v5l3 2" /></>,
  };
  return <svg className={`usage-tone-icon ${type}`} viewBox="0 0 22 22" aria-hidden="true">{paths[type]}</svg>;
}

function TokenRail({ aggregate }: { aggregate: AggView }) {
  const { language, copy } = useLocalizedCopy();
  const total = aggregate.input_tokens + aggregate.output_tokens;
  const inputPercent = total ? (aggregate.input_tokens / total) * 100 : 0;
  const outputPercent = total ? 100 - inputPercent : 0;
  const cachePercent = aggregate.input_tokens
    ? Math.min(100, (aggregate.cache_read_tokens / aggregate.input_tokens) * 100)
    : 0;
  return (
    <div
      className="usage-token-rail"
      role="group"
      aria-label={copy("Token composition", "Token 构成", "Token 組成", "トークンの構成")}
    >
      <div className="usage-composition-head">
        <div>
          <span>{copy("Token composition", "Token 构成", "Token 組成", "トークンの構成")}</span>
          <small>{copy("Input and output form the total", "输入与输出组成总量", "輸入與輸出組成總量", "入力と出力が総量を形成します")}</small>
        </div>
        <p>{copy(
          "Total Tokens = input + output; cache and reasoning are nested and are not counted twice.",
          "总 Token = 输入 + 输出；缓存和推理为子项，不重复计数。", "總 Token = 輸入 + 輸出；快取和推理為子項，不重複計數。", "総トークン = 入力 + 出力；キャッシュと推論はサブ項目で、重複してカウントされません。"
        )}</p>
      </div>
      <div className="usage-rail-labels">
        <span><i className="tone-input" /><em>{copy("Input", "输入", "輸入", "入力")}</em><strong>{compact(aggregate.input_tokens, language)}</strong></span>
        <span><i className="tone-output" /><em>{copy("Output", "输出", "輸出", "出力")}</em><strong>{compact(aggregate.output_tokens, language)}</strong></span>
      </div>
      <div
        className="usage-rail-track"
        aria-label={total
          ? copy(
              `Input ${inputPercent.toFixed(1)}%, output ${outputPercent.toFixed(1)}%`,
              `输入占 ${inputPercent.toFixed(1)}%，输出占 ${outputPercent.toFixed(1)}%`, `輸入 ${inputPercent.toFixed(1)}%，輸出 ${outputPercent.toFixed(1)}%`, `入力 ${inputPercent.toFixed(1)}%，出力 ${outputPercent.toFixed(1)}%`
            )
          : copy("No token data", "暂无 Token 数据", "暫無 Token 資料", "トークンデータはありません")}
      >
        <div className="usage-rail-input" style={{ width: `${inputPercent}%` }}>
          <span className="usage-rail-cache" style={{ width: `${cachePercent}%` }} />
        </div>
        <div className="usage-rail-output" style={{ width: `${outputPercent}%` }} />
      </div>
      <div className="usage-token-details">
        <div>
          <span><i className="tone-cache" />{copy("Cache read", "缓存读", "快取讀取", "キャッシュ読み込み")}</span>
          <strong>{compact(aggregate.cache_read_tokens, language)}</strong>
        </div>
        <div>
          <span><i className="tone-cache-write" />{copy("Cache write", "缓存写", "快取寫入", "キャッシュ書き込み")}</span>
          <strong>{compact(aggregate.cache_write_tokens, language)}</strong>
        </div>
        <div>
          <span><i className="tone-reasoning" />{copy("Reasoning", "推理", "推理", "推論")}</span>
          <strong>{compact(aggregate.reasoning_tokens, language)}</strong>
        </div>
        <div className="usage-cache-rate">
          <span>{copy("Cache reuse rate", "缓存复用率", "快取復用率", "キャッシュ再利用率")}</span>
          <strong>{cacheRate(aggregate)}</strong>
        </div>
      </div>
    </div>
  );
}

export default function Stats({ onBack, embedded = false }: { onBack?: () => void; embedded?: boolean }) {
  const { language, copy } = useLocalizedCopy();
  const { showError } = useErrorToast();
  const sinceOptions = [
    { value: "24h", label: copy("Last 24 hours", "近 24 小时", "近 24 小時", "過去 24 時間") },
    { value: "7d", label: copy("Last 7 days", "近 7 天", "近 7 天", "過去 7 日間") },
    { value: "30d", label: copy("Last 30 days", "近 30 天", "近 30 天", "過去 30 日間") },
    { value: "all", label: copy("All time", "全部历史", "全部歷史", "すべての履歴") },
  ];
  const groups: Array<{ value: GroupValue; label: string; shortLabel: string }> = [
    { value: "agent", label: copy("By Agent", "按 Agent", "按 Agent", "Agent ごと"), shortLabel: "Agent" },
    { value: "upstream", label: copy("By provider", "按供应商", "按供應商", "プロバイダーごと"), shortLabel: copy("Provider", "供应商", "供應商", "プロバイダー") },
    { value: "model", label: copy("By model", "按模型", "按模型", "モデルごと"), shortLabel: copy("Model", "模型", "模型", "モデル") },
    { value: "status", label: copy("By status", "按状态", "按狀態", "ステータスごと"), shortLabel: copy("Status", "状态", "狀態", "ステータス") },
    // Which transport carried each attempt, and — when one did not use South —
    // why. Together they answer "is my traffic actually on the new path", which
    // the totals alone cannot.
    { value: "engine", label: copy("By engine", "按引擎", "按引擎", "エンジンごと"), shortLabel: copy("Engine", "引擎", "引擎", "エンジン") },
    { value: "fallback", label: copy("By fallback reason", "按回退原因", "按回退原因", "フォールバック理由ごと"), shortLabel: copy("Fallback", "回退原因", "回退原因", "フォールバック") },
  ];
  const [since, setSince] = useState("24h");
  const [activeGroup, setActiveGroup] = useState<GroupValue>("agent");
  const [agentFilter, setAgentFilter] = useState("");
  const [upstreamFilter, setUpstreamFilter] = useState("");
  const [modelFilter, setModelFilter] = useState("");
  const [refreshInterval, setRefreshInterval] = useState(0);
  const [filtersOpen, setFiltersOpen] = useState(false);
  const [data, setData] = useState<StatsView | null>(null);
  const [trend, setTrend] = useState<StatsView | null>(null);
  const [upstreams, setUpstreams] = useState<string[]>([]);
  const [models, setModels] = useState<string[]>([]);
  const [err, setErr] = useState("");
  const [loading, setLoading] = useState(true);
  const [refreshing, setRefreshing] = useState(false);
  const requestGeneration = useRef(0);
  const dashboardInFlight = useRef(false);
  const dashboardQueued = useRef(false);
  const activeDashboardKey = useRef("");
  const dashboardMounted = useRef(true);
  const latestDashboardLoader = useRef<(background?: boolean) => Promise<void>>(async () => undefined);

  const [agents, setAgents] = useState<AgentUiMetadataView[]>([]);
  const [budgets, setBudgets] = useState<BudgetStatus[]>([]);

  useEffect(() => {
    dashboardMounted.current = true;
    return () => {
      dashboardMounted.current = false;
      dashboardQueued.current = false;
      requestGeneration.current += 1;
    };
  }, []);

  useEffect(() => {
    Promise.all([listAgentRegistry(), getAgentBudgets()])
      .then(([registry, statuses]) => {
        const supported = registry.filter((agent) => agent.admission === "supported");
        setAgents(supported);
        setBudgets(statuses);
      })
      .catch((error) => showError(humanizeAppError(error), "usage-budget-status"));
  }, [showError]);

  const loadDashboard = useCallback(async (background = false) => {
    const requestKey = JSON.stringify([
      since,
      activeGroup,
      agentFilter,
      upstreamFilter,
      modelFilter,
    ]);
    if (dashboardInFlight.current) {
      dashboardQueued.current = true;
      if (requestKey !== activeDashboardKey.current) requestGeneration.current += 1;
      return;
    }
    const generation = ++requestGeneration.current;
    dashboardInFlight.current = true;
    activeDashboardKey.current = requestKey;
    if (background || data) setRefreshing(true);
    else setLoading(true);
    setErr("");
    const trendBy = since === "24h" ? "hour" : "day";
    try {
      const settled = await Promise.allSettled([
        getStats(since, activeGroup, agentFilter || null, null, upstreamFilter || null, modelFilter || null),
        getStats(since, trendBy, agentFilter || null, null, upstreamFilter || null, modelFilter || null),
        getStats(since, "upstream", agentFilter || null, null, null, null),
        getStats(since, "model", agentFilter || null, null, upstreamFilter || null, null),
      ]);
      const failure = settled.find(
        (result): result is PromiseRejectedResult => result.status === "rejected",
      );
      if (failure) throw failure.reason;
      const [nextData, nextTrend, upstreamData, modelData] = settled.map(
        (result) => (result as PromiseFulfilledResult<StatsView>).value,
      );
      if (!dashboardMounted.current || generation !== requestGeneration.current) return;
      const nextUpstreams = upstreamData.groups.map(([name]) => name).filter((name) => name !== "(unrouted)");
      const nextModels = modelData.groups.map(([name]) => name).filter((name) => name !== "(unrouted)");
      setData(nextData);
      setTrend(nextTrend);
      setUpstreams(nextUpstreams);
      setModels(nextModels);
      if (upstreamFilter && !nextUpstreams.includes(upstreamFilter)) {
        setUpstreamFilter("");
        setModelFilter("");
      } else if (modelFilter && !nextModels.includes(modelFilter)) {
        setModelFilter("");
      }
    } catch (error) {
      if (dashboardMounted.current && generation === requestGeneration.current) {
        const message = humanizeAppError(error);
        if (background || data) showError(message, "usage-dashboard-refresh");
        else setErr(message);
      }
    } finally {
      if (dashboardMounted.current && generation === requestGeneration.current) {
        setLoading(false);
        setRefreshing(false);
      }
      dashboardInFlight.current = false;
      if (dashboardMounted.current && dashboardQueued.current) {
        dashboardQueued.current = false;
        queueMicrotask(() => {
          if (dashboardMounted.current) void latestDashboardLoader.current(true);
        });
      }
    }
  }, [activeGroup, agentFilter, data, modelFilter, showError, since, upstreamFilter]);
  latestDashboardLoader.current = loadDashboard;

  useEffect(() => {
    void loadDashboard();
    // `data` only decides whether to show initial or background loading and
    // must not turn a successful response into a fetch loop.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeGroup, agentFilter, modelFilter, since, upstreamFilter]);

  useEffect(() => {
    if (!refreshInterval) return undefined;
    const timer = window.setInterval(() => void loadDashboard(true), refreshInterval);
    return () => window.clearInterval(timer);
  }, [loadDashboard, refreshInterval]);

  const aggregate = data?.total ?? EMPTY_AGG;
  const totalTokens = aggregate.input_tokens + aggregate.output_tokens;
  const hasFilters = Boolean(agentFilter || upstreamFilter || modelFilter);
  const displayName = (id: string) =>
    agents.find((agent) => agent.agent_id === id)?.display_name ?? id;
  const selectedBudgets = agentFilter
    ? budgets.filter((budget) => budget.agent_id === agentFilter)
    : budgets;
  const groupLabel = groups.find((group) => group.value === activeGroup)?.label
    ?? copy("Details", "明细", "明細", "詳細");
  const visibleUpstreams = useMemo(() => upstreams, [upstreams]);
  const visibleModels = useMemo(() => models, [models]);

  return (
    <section className={`usage-page ${embedded ? "usage-page-embedded" : ""}`}>
      {!embedded && <header className="usage-page-head">
        <div>
          {onBack && <PageBackButton onClick={onBack} />}
          <h1>{copy("Usage", "用量统计", "用量", "使用状況")}</h1>
          <p>{copy(
            "Review model usage, cost, and reliability. Only local request metadata is aggregated; prompt and response content is never included.",
            "查看 AI 模型的消耗、成本与稳定性。只聚合本地请求元数据，不含 prompt 或 response 内容。", "檢視模型用量、成本與穩定性。只彙整本機請求的中繼資料，不包含提示詞或回應內容。", "モデルの使用状況、コスト、信頼性を確認します。集計対象はローカルのリクエストメタデータのみで、プロンプトやレスポンスの内容は含まれません。"
          )}</p>
        </div>
      </header>}

      <div className="usage-filter-disclosure">
        <button
          className="usage-filter-toggle"
          type="button"
          aria-label={copy("Filters", "筛选", "篩選", "フィルター")}
          aria-expanded={filtersOpen}
          aria-controls="usage-filter-panel"
          onClick={() => setFiltersOpen((current) => !current)}
        >
          <SlidersHorizontal aria-hidden="true" />
          <strong>{copy("Filters", "筛选", "篩選", "フィルター")}</strong>
          <span aria-hidden="true">{sinceOptions.find((range) => range.value === since)?.label}</span>
          {hasFilters && <em aria-hidden="true">{[agentFilter, upstreamFilter, modelFilter].filter(Boolean).length}</em>}
        </button>
        {hasFilters && (
          <button className="usage-clear-filters" type="button" onClick={() => { setAgentFilter(""); setUpstreamFilter(""); setModelFilter(""); }}>
            {copy("Clear filters", "清除筛选", "清除篩選", "フィルタをクリア")}
          </button>
        )}
      </div>

      {filtersOpen && <div id="usage-filter-panel" className="usage-toolbar" aria-label={copy("Usage filters", "用量筛选", "用量篩選", "使用状況フィルター")}>
        <div className="usage-filter-field">
          <span>Agent</span>
          <CompactCombobox
            ariaLabel={copy("Agent filter", "Agent 过滤", "Agent 篩選", "Agent フィルター")}
            value={agentFilter}
            options={[
              { value: "", label: copy("All Agents", "全部 Agent", "全部 Agent", "すべての Agent") },
              ...agents.map((agent) => ({ value: agent.agent_id, label: agent.display_name })),
            ]}
            onChange={(value) => {
              setAgentFilter(value);
              setUpstreamFilter("");
              setModelFilter("");
            }}
          />
        </div>
        <div className="usage-filter-field">
          <span>{copy("Provider", "供应商", "供應商", "プロバイダー")}</span>
          <CompactCombobox
            ariaLabel={copy("Provider filter", "供应商过滤", "供應商篩選", "プロバイダー フィルター")}
            value={upstreamFilter}
            options={[
              { value: "", label: copy("All providers", "全部供应商", "所有供應商", "すべてのプロバイダー") },
              ...visibleUpstreams.map((upstream) => ({ value: upstream, label: upstream })),
            ]}
            onChange={(value) => {
              setUpstreamFilter(value);
              setModelFilter("");
            }}
          />
        </div>
        <div className="usage-filter-field">
          <span>{copy("Model", "模型", "模型", "モデル")}</span>
          <CompactCombobox
            ariaLabel={copy("Model filter", "模型过滤", "模型過濾", "モデルフィルター")}
            value={modelFilter}
            options={[
              { value: "", label: copy("All models", "全部模型", "所有模型", "すべてのモデル") },
              ...visibleModels.map((model) => ({ value: model, label: model })),
            ]}
            onChange={setModelFilter}
          />
        </div>
        <div className="usage-refresh-control">
          <div className="usage-filter-field usage-refresh-select">
            <span>{copy("Auto-refresh", "自动刷新", "自動重新整理", "自動リフレッシュ")}</span>
            <CompactCombobox
              ariaLabel={copy("Auto-refresh", "自动刷新", "自動重新整理", "自動リフレッシュ")}
              value={String(refreshInterval)}
              options={[
                { value: "0", label: copy("Off", "关闭", "關閉", "オフ") },
                { value: "30000", label: copy("30 seconds", "30 秒", "30 秒", "30 秒") },
                { value: "60000", label: copy("60 seconds", "60 秒", "60 秒", "60 秒") },
              ]}
              onChange={(value) => setRefreshInterval(Number(value))}
            />
          </div>
          <button
            className={`usage-refresh-button ${refreshing ? "busy" : ""}`}
            type="button"
            aria-label={copy("Refresh usage", "刷新用量", "重新整理用量", "使用状況を更新")}
            title={copy("Refresh usage", "刷新用量", "重新整理用量", "使用状況を更新")}
            disabled={refreshing}
            onClick={() => void loadDashboard(true)}
          >
            <span aria-hidden="true">↻</span>
          </button>
        </div>
        <div className="usage-filter-field usage-range-select">
          <span>{copy("Time range", "时间范围", "時間範圍", "時間範囲")}</span>
          <CompactCombobox
            ariaLabel={copy("Time range", "时间范围", "時間範圍", "時間範囲")}
            value={since}
            options={sinceOptions}
            onChange={setSince}
          />
        </div>
      </div>}

      {err && <div className="banner err usage-error">{err}</div>}

      {loading ? (
        <div className="usage-loading" aria-label={copy("Loading usage", "正在加载用量", "正在載入用量", "使用状況を読み込み中")}>
          <div /><div /><div />
        </div>
      ) : data?.empty ? (
        <div className="usage-empty panel">
          <span aria-hidden="true">⌁</span>
          <h2>{copy("No local usage records yet", "还没有本地用量记录", "還沒有本地用量記錄", "まだローカルの使用状況記録がありません")}</h2>
          <p>{copy(
            "Enable Local metrics in Settings, start the proxy, and complete a model request to create usage statistics.",
            "在“设置 · 本地指标”中开启记录，启动代理并完成一次模型请求后，这里会生成统计。", "在「設定 · 本地指標」中啟用記錄，啟動代理並完成一次模型請求後，這裡會生成統計。", "「設定 · ローカルメトリクス」で記録を有効にし、プロキシを起動してモデルリクエストを1回実行すると、ここに統計が生成されます。"
          )}</p>
        </div>
      ) : (
        <>
          <section className="usage-hero" aria-label={copy("Usage overview", "用量总览", "用量總覽", "使用状況概要")}>
            <div className="usage-primary-metric">
              <ToneIcon type="token" />
              <div>
                <span>{copy("Total tokens", "总 Token", "總 Token", "合計トークン")}</span>
                <strong title={totalTokens.toLocaleString(language)}>{totalTokens.toLocaleString(language)}</strong>
                <small>≈ {compact(totalTokens, language, 2)} · {copy("input + output", "输入 + 输出", "輸入 + 輸出", "入力 + 出力")}</small>
              </div>
            </div>
            <div className="usage-kpi-grid">
              <div><ToneIcon type="request" /><span>{copy("Requests", "请求", "請求", "リクエスト")}</span><strong>{aggregate.requests.toLocaleString(language)}</strong></div>
              <div><ToneIcon type="cost" /><span>{copy("Estimated cost", "估算成本", "估算成本", "推定コスト")}</span><strong>{cost(aggregate.cost_micros)}</strong></div>
              <div><ToneIcon type="success" /><span>{copy("Success rate", "成功率", "成功率", "成功率")}</span><strong>{successRate(aggregate)}</strong></div>
              <div><ToneIcon type="latency" /><span>{copy("p95 latency", "p95 延迟", "p95 延遲", "p95 レイテンシー")}</span><strong>{latency(aggregate.p95_latency_ms)}</strong></div>
            </div>
            <TokenRail aggregate={aggregate} />
            {aggregate.unpriced_requests > 0 && (
              <div className="usage-unpriced-note">
                {copy(
                  `Known cost covers ${aggregate.priced_requests} requests; ${aggregate.unpriced_requests} requests are unpriced.`,
                  `已知成本覆盖 ${aggregate.priced_requests} 个请求；另有 ${aggregate.unpriced_requests} 个请求未定价。`, `已知成本覆蓋 ${aggregate.priced_requests} 個請求；另有 ${aggregate.unpriced_requests} 個請求未定價。`, `既知のコストは ${aggregate.priced_requests} 個のリクエストをカバー；${aggregate.unpriced_requests} 個のリクエストは価格が未設定です。`
                )}
              </div>
            )}
          </section>

          <section className="usage-trend-panel">
            <header>
              <div>
                <div>
                  <h2>{copy("Usage trend", "使用趋势", "使用趨勢", "使用トレンド")}</h2>
                  <p>{sinceOptions.find((range) => range.value === since)?.label} · {since === "24h"
                    ? copy("Hourly", "按小时", "按小時", "時間単位")
                    : copy("Daily", "按天", "按天", "日単位")}</p>
                </div>
              </div>
              <span className="usage-dual-axis-note">{copy("Left: tokens · Right: cost", "左轴 Token · 右轴成本", "左軸 Token · 右軸成本", "左軸 Token · 右軸コスト")}</span>
            </header>
            <div className="usage-chart-legend">
              <span><i className="tone-input" />{copy("Input", "输入", "輸入", "入力")}</span>
              <span><i className="tone-output" />{copy("Output", "输出", "輸出", "出力")}</span>
              <span><i className="tone-cache-write" />{copy("Cache write", "缓存写入", "快取寫入", "キャッシュ書き込み")}</span>
              <span><i className="tone-cache-read" />{copy("Cache hit", "缓存命中", "快取命中", "キャッシュヒット")}</span>
              <span><i className="tone-cost" />{copy("Cost", "成本", "成本", "コスト")}</span>
            </div>
            <UsageTrendChart groups={trend?.groups ?? []} range={since as UsageTrendRange} />
          </section>

          <section className="usage-detail-panel">
            <header>
              <div>
                <div><h2>{copy("Contribution details", "贡献明细", "貢獻明細", "貢献明細")}</h2><p>{groupLabel} · {copy("Current filters", "当前筛选范围", "當前篩選範圍", "現在のフィルタ")}</p></div>
              </div>
              <div className="usage-segmented" role="tablist" aria-label={copy("Aggregation view", "统计视角", "統計視角", "統計ビュー")}>
                {groups.map((group) => (
                  <button
                    key={group.value}
                    type="button"
                    role="tab"
                    aria-selected={activeGroup === group.value}
                    className={activeGroup === group.value ? "active" : ""}
                    onClick={() => setActiveGroup(group.value)}
                  >
                    {group.shortLabel}
                  </button>
                ))}
              </div>
            </header>
            <div className="usage-table-wrap">
              <table className="usage-table">
                <thead><tr><th>{groupLabel}</th><th>{copy("Requests", "请求", "請求", "リクエスト")}</th><th>{copy("Success rate", "成功率", "成功率", "成功率")}</th><th>Token</th><th>p95</th><th>{copy("Cost", "成本", "成本", "コスト")}</th></tr></thead>
                <tbody>
                  {(data?.groups ?? []).map(([name, item]) => (
                    <tr key={name}>
                      <td><span className="usage-row-mark" />{activeGroup === "agent" ? displayName(name) : name}</td>
                      <td>{item.requests.toLocaleString()}</td>
                      <td>{successRate(item)}</td>
                      <td title={(item.input_tokens + item.output_tokens).toLocaleString(language)}>{compact(item.input_tokens + item.output_tokens, language)}</td>
                      <td>{latency(item.p95_latency_ms)}</td>
                      <td>{cost(item.cost_micros)}{item.unpriced_requests > 0 && (
                        <small title={copy(
                          `${item.unpriced_requests} unpriced requests`,
                          `${item.unpriced_requests} 个请求未定价`, `${item.unpriced_requests} 個請求未定價`, `${item.unpriced_requests} 個のリクエストは価格が未設定`
                        )}> +?</small>
                      )}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
              {(data?.groups.length ?? 0) === 0 && (
                <div className="usage-table-empty">{copy(
                  "No groups to display for this view.",
                  "当前视角没有可显示的分组。", "當前視角沒有可顯示的分組。", "現在のビューには表示可能なグループがありません。"
                )}</div>
              )}
            </div>
          </section>

        </>
      )}

      {selectedBudgets.length > 0 && (
        <section className="usage-budget-overview">
          <header>
            <div><div><h2>{copy("Budget status", "预算状态", "預算狀態", "予算状態")}</h2><p>{copy("Alerts only; routing is unchanged", "仅提醒，不影响路由", "僅提醒；路由未變更", "アラートのみ；ルーティングは変更されていません")}</p></div></div>
            <span className="budget-observe-badge">OBSERVE ONLY</span>
          </header>
          <div className="usage-budget-list">
            {selectedBudgets.map((budget) => {
              const percent = Math.min(100, Math.max(0, budget.usage_percent));
              const warning = budget.usage_level !== "healthy" || ["expiring", "expired"].includes(budget.expiry_level);
              return (
                <div className={`usage-budget-row ${warning ? "warn" : ""}`} key={budget.agent_id}>
                  <div>
                    <strong>{budgetWarning(budget, displayName(budget.agent_id), copy)}</strong>
                    <small>{copy(
                      `Remaining ${formatBudgetAmount(budget.remaining_micros)} / Limit ${formatBudgetAmount(budget.limit_micros)}`,
                      `剩余 ${formatBudgetAmount(budget.remaining_micros)} / 上限 ${formatBudgetAmount(budget.limit_micros)}`, `剩餘 ${formatBudgetAmount(budget.remaining_micros)} / 上限 ${formatBudgetAmount(budget.limit_micros)}`, `残り ${formatBudgetAmount(budget.remaining_micros)} / 上限 ${formatBudgetAmount(budget.limit_micros)}`
                    )}</small>
                  </div>
                  <div className="usage-budget-progress"><span style={{ width: `${percent}%` }} /></div>
                  <em>{budget.usage_percent.toFixed(1)}%</em>
                </div>
              );
            })}
          </div>
        </section>
      )}

    </section>
  );
}
