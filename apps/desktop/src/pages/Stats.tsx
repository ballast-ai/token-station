import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  AgentUiMetadataView,
  AggView,
  BudgetStatus,
  StatsView,
  getAgentBudgets,
  getStats,
  listAgentRegistry,
  removeAgentBudget,
  setAgentBudget,
} from "../api";
import PricingEditor from "../components/PricingEditor";
import PageBackButton from "../components/PageBackButton";
import CompactCombobox from "../components/CompactCombobox";
import UsageTrendChart, { type UsageTrendRange } from "../components/UsageTrendChart";
import { useLocalizedCopy } from "../components/LanguageProvider";
import { humanizeAppError } from "../errors";
import { useErrorToast } from "../components/ErrorToast";

export function formatBudgetAmount(micros: number): string {
  if (micros === 0) return "0.00";
  const amount = micros / 1_000_000;
  if (Math.abs(amount) >= 0.01) return amount.toFixed(2);
  return amount.toFixed(6).replace(/0+$/, "").replace(/\.$/, "");
}

type GroupValue = "agent" | "upstream" | "model" | "status";

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

function localDateTime(ms: number | null): string {
  if (ms == null) return "";
  const date = new Date(ms);
  const shifted = new Date(date.getTime() - date.getTimezoneOffset() * 60_000);
  return shifted.toISOString().slice(0, 16);
}

function budgetWarning(
  status: BudgetStatus,
  name: string,
  copy: (english: string, simplifiedChinese: string) => string,
): string {
  const parts: string[] = [];
  if (status.usage_level === "approaching") {
    parts.push(copy(
      `${name} has used ${status.usage_percent.toFixed(1)}% and is approaching the budget limit`,
      `${name} 已使用 ${status.usage_percent.toFixed(1)}%，接近预算上限`,
    ));
  } else if (status.usage_level === "exceeded") {
    parts.push(copy(
      `${name} has used ${status.usage_percent.toFixed(1)}% and exceeded the budget limit`,
      `${name} 已使用 ${status.usage_percent.toFixed(1)}%，已超过预算上限`,
    ));
  } else if (status.usage_level === "unknown") {
    parts.push(copy(
      `${name} has ${status.unpriced_requests} unpriced requests, so budget usage is incomplete`,
      `${name} 有 ${status.unpriced_requests} 个请求未定价，预算用量不完整`,
    ));
  } else {
    parts.push(copy(
      `${name} has used ${status.usage_percent.toFixed(1)}%`,
      `${name} 已使用 ${status.usage_percent.toFixed(1)}%`,
    ));
  }
  if (status.expiry_level === "expiring") parts.push(copy("Budget period expires soon", "预算周期即将到期"));
  if (status.expiry_level === "expired") parts.push(copy("Budget period expired", "预算周期已到期"));
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
    <div className="usage-token-rail">
      <div className="usage-rail-labels">
        <span><i className="tone-input" />{copy("Input", "输入")} <strong>{compact(aggregate.input_tokens, language)}</strong></span>
        <span><i className="tone-output" />{copy("Output", "输出")} <strong>{compact(aggregate.output_tokens, language)}</strong></span>
      </div>
      <div
        className="usage-rail-track"
        aria-label={total
          ? copy(
              `Input ${inputPercent.toFixed(1)}%, output ${outputPercent.toFixed(1)}%`,
              `输入占 ${inputPercent.toFixed(1)}%，输出占 ${outputPercent.toFixed(1)}%`,
            )
          : copy("No token data", "暂无 Token 数据")}
      >
        <div className="usage-rail-input" style={{ width: `${inputPercent}%` }}>
          <span className="usage-rail-cache" style={{ width: `${cachePercent}%` }} />
        </div>
        <div className="usage-rail-output" style={{ width: `${outputPercent}%` }} />
      </div>
      <div className="usage-rail-foot">
        <span><i className="tone-cache" />{copy("Cache hits cover", "缓存命中覆盖输入的")} <strong>{cacheRate(aggregate)}</strong> {copy("of input", "")}</span>
        <span>{copy(
          `Cache read ${compact(aggregate.cache_read_tokens, language)} · Cache write ${compact(aggregate.cache_write_tokens, language)} · Reasoning ${compact(aggregate.reasoning_tokens, language)}`,
          `缓存读 ${compact(aggregate.cache_read_tokens, language)} · 缓存写 ${compact(aggregate.cache_write_tokens, language)} · 推理 ${compact(aggregate.reasoning_tokens, language)}`,
        )}</span>
        <small>{copy(
          "Cache is part of input and reasoning is part of output; neither is counted twice in total tokens.",
          "缓存属于输入、推理属于输出，均不重复计入总 Token。",
        )}</small>
      </div>
    </div>
  );
}

export default function Stats({ onBack, embedded = false }: { onBack?: () => void; embedded?: boolean }) {
  const { language, copy } = useLocalizedCopy();
  const { showError, showSuccess } = useErrorToast();
  const sinceOptions = [
    { value: "24h", label: copy("Last 24 hours", "近 24 小时") },
    { value: "7d", label: copy("Last 7 days", "近 7 天") },
    { value: "30d", label: copy("Last 30 days", "近 30 天") },
    { value: "all", label: copy("All time", "全部历史") },
  ];
  const groups: Array<{ value: GroupValue; label: string; shortLabel: string }> = [
    { value: "agent", label: copy("By Agent", "按 Agent"), shortLabel: "Agent" },
    { value: "upstream", label: copy("By provider", "按供应商"), shortLabel: copy("Provider", "供应商") },
    { value: "model", label: copy("By model", "按模型"), shortLabel: copy("Model", "模型") },
    { value: "status", label: copy("By status", "按状态"), shortLabel: copy("Status", "状态") },
  ];
  const [since, setSince] = useState("24h");
  const [activeGroup, setActiveGroup] = useState<GroupValue>("agent");
  const [agentFilter, setAgentFilter] = useState("");
  const [upstreamFilter, setUpstreamFilter] = useState("");
  const [modelFilter, setModelFilter] = useState("");
  const [refreshInterval, setRefreshInterval] = useState(0);
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
  const [budgetErr, setBudgetErr] = useState("");
  const [agentId, setAgentId] = useState("");
  const [limit, setLimit] = useState("10");
  const [warningPercent, setWarningPercent] = useState("80");
  const [periodStart, setPeriodStart] = useState("");
  const [periodEnd, setPeriodEnd] = useState("");
  const [expiryWarningDays, setExpiryWarningDays] = useState("7");

  const loadForm = useCallback((selected: string, statuses: BudgetStatus[]) => {
    const status = statuses.find((candidate) => candidate.agent_id === selected);
    setLimit(status ? String(status.limit_micros / 1_000_000) : "10");
    setWarningPercent(status ? String(status.warning_percent) : "80");
    setPeriodStart(status ? localDateTime(status.period_start_ms) : "");
    setPeriodEnd(status ? localDateTime(status.period_end_ms) : "");
    setExpiryWarningDays(status ? String(status.expiry_warning_days) : "7");
  }, []);

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
        const selected = supported[0]?.agent_id ?? "";
        setAgentId(selected);
        loadForm(selected, statuses);
      })
      .catch((error) => setBudgetErr(humanizeAppError(error)));
  }, [loadForm]);

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

  const saveBudget = async () => {
    setBudgetErr("");
    const limitValue = Number(limit);
    const limitMicros = Math.round(limitValue * 1_000_000);
    const warning = Number(warningPercent);
    const expiryDays = Number(expiryWarningDays);
    const startMs = periodStart ? new Date(periodStart).getTime() : null;
    const endMs = periodEnd ? new Date(periodEnd).getTime() : null;
    if (!agentId
        || !/^\d+(?:\.\d{1,6})?$/.test(limit)
        || !Number.isFinite(limitValue)
        || limitValue > 9_000_000_000
        || !Number.isSafeInteger(limitMicros)
        || limitMicros <= 0) {
      setBudgetErr(copy(
        "Budget limit must be greater than 0, no more than 9 billion, and use at most 6 decimal places.",
        "预算上限必须是大于 0、不超过 90 亿且最多 6 位小数的金额。",
      ));
      return;
    }
    if (!Number.isInteger(warning) || warning < 1 || warning > 100) {
      setBudgetErr(copy("Warning threshold must be an integer from 1 to 100.", "预警阈值必须是 1–100 的整数。"));
      return;
    }
    if (!Number.isInteger(expiryDays) || expiryDays < 0 || expiryDays > 365) {
      setBudgetErr(copy("Expiry warning days must be an integer from 0 to 365.", "到期预警天数必须是 0–365 的整数。"));
      return;
    }
    if ((startMs != null && !Number.isFinite(startMs)) || (endMs != null && !Number.isFinite(endMs))) {
      setBudgetErr(copy("Budget period date is invalid.", "预算周期时间无效。"));
      return;
    }
    if (startMs != null && endMs != null && startMs >= endMs) {
      setBudgetErr(copy("Budget period end must be later than its start.", "预算周期结束时间必须晚于开始时间。"));
      return;
    }
    try {
      const statuses = await setAgentBudget(agentId, limitMicros, warning, startMs, endMs, expiryDays);
      setBudgets(statuses);
      loadForm(agentId, statuses);
      showSuccess(
        copy("Budget saved · Alerts only", "预算已保存 · 仅用于展示与预警"),
        `agent-budget-save:${agentId}`,
      );
    } catch (error) {
      showError(humanizeAppError(error), `agent-budget-save:${agentId}`);
    }
  };

  const deleteBudget = async () => {
    setBudgetErr("");
    try {
      const statuses = await removeAgentBudget(agentId);
      setBudgets(statuses);
      loadForm(agentId, statuses);
      showSuccess(copy("Budget deleted", "预算已删除"), `agent-budget-remove:${agentId}`);
    } catch (error) {
      showError(humanizeAppError(error), `agent-budget-remove:${agentId}`);
    }
  };

  const aggregate = data?.total ?? EMPTY_AGG;
  const totalTokens = aggregate.input_tokens + aggregate.output_tokens;
  const hasFilters = Boolean(agentFilter || upstreamFilter || modelFilter);
  const displayName = (id: string) =>
    agents.find((agent) => agent.agent_id === id)?.display_name ?? id;
  const selectedBudgets = agentFilter
    ? budgets.filter((budget) => budget.agent_id === agentFilter)
    : budgets;
  const hasSelectedBudget = budgets.some((budget) => budget.agent_id === agentId);
  const groupLabel = groups.find((group) => group.value === activeGroup)?.label
    ?? copy("Details", "明细");
  const visibleUpstreams = useMemo(() => upstreams, [upstreams]);
  const visibleModels = useMemo(() => models, [models]);

  return (
    <section className={`usage-page ${embedded ? "usage-page-embedded" : ""}`}>
      {!embedded && <header className="usage-page-head">
        <div>
          {onBack && <PageBackButton onClick={onBack} />}
          <h1>{copy("Usage", "用量统计")}</h1>
          <p>{copy(
            "Review model usage, cost, and reliability. Only local request metadata is aggregated; prompt and response content is never included.",
            "查看 AI 模型的消耗、成本与稳定性。只聚合本地请求元数据，不含 prompt 或 response 内容。",
          )}</p>
        </div>
      </header>}

      <div className="usage-toolbar" aria-label={copy("Usage filters", "用量筛选")}>
        <div className="usage-filter-field">
          <span>Agent</span>
          <CompactCombobox
            ariaLabel={copy("Agent filter", "Agent 过滤")}
            value={agentFilter}
            options={[
              { value: "", label: copy("All Agents", "全部 Agent") },
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
          <span>{copy("Provider", "供应商")}</span>
          <CompactCombobox
            ariaLabel={copy("Provider filter", "供应商过滤")}
            value={upstreamFilter}
            options={[
              { value: "", label: copy("All providers", "全部供应商") },
              ...visibleUpstreams.map((upstream) => ({ value: upstream, label: upstream })),
            ]}
            onChange={(value) => {
              setUpstreamFilter(value);
              setModelFilter("");
            }}
          />
        </div>
        <div className="usage-filter-field">
          <span>{copy("Model", "模型")}</span>
          <CompactCombobox
            ariaLabel={copy("Model filter", "模型过滤")}
            value={modelFilter}
            options={[
              { value: "", label: copy("All models", "全部模型") },
              ...visibleModels.map((model) => ({ value: model, label: model })),
            ]}
            onChange={setModelFilter}
          />
        </div>
        <div className="usage-toolbar-spacer" />
        <div className="usage-filter-field usage-refresh-select">
          <span>{copy("Auto-refresh", "自动刷新")}</span>
          <CompactCombobox
            ariaLabel={copy("Auto-refresh", "自动刷新")}
            value={String(refreshInterval)}
            options={[
              { value: "0", label: copy("Off", "关闭") },
              { value: "30000", label: copy("30 seconds", "30 秒") },
              { value: "60000", label: copy("60 seconds", "60 秒") },
            ]}
            onChange={(value) => setRefreshInterval(Number(value))}
          />
        </div>
        <button
          className={`usage-refresh-button ${refreshing ? "busy" : ""}`}
          type="button"
          aria-label={copy("Refresh usage", "刷新用量")}
          title={copy("Refresh usage", "刷新用量")}
          disabled={refreshing}
          onClick={() => void loadDashboard(true)}
        >
          <span aria-hidden="true">↻</span>
        </button>
        <div className="usage-filter-field usage-range-select">
          <span>{copy("Time range", "时间范围")}</span>
          <CompactCombobox
            ariaLabel={copy("Time range", "时间范围")}
            value={since}
            options={sinceOptions}
            onChange={setSince}
          />
        </div>
      </div>

      {err && <div className="banner err usage-error">{err}</div>}

      {loading ? (
        <div className="usage-loading" aria-label={copy("Loading usage", "正在加载用量")}>
          <div /><div /><div />
        </div>
      ) : data?.empty ? (
        <div className="usage-empty panel">
          <span aria-hidden="true">⌁</span>
          <h2>{copy("No local usage records yet", "还没有本地用量记录")}</h2>
          <p>{copy(
            "Enable Local metrics in Settings, start the proxy, and complete a model request to create usage statistics.",
            "在“设置 · 本地指标”中开启记录，启动代理并完成一次模型请求后，这里会生成统计。",
          )}</p>
        </div>
      ) : (
        <>
          <section className="usage-hero" aria-label={copy("Usage overview", "用量总览")}>
            <div className="usage-primary-metric">
              <ToneIcon type="token" />
              <div>
                <span>{copy("Total tokens", "总 Token")}</span>
                <strong title={totalTokens.toLocaleString(language)}>{totalTokens.toLocaleString(language)}</strong>
                <small>≈ {compact(totalTokens, language, 2)} · {copy("input + output", "输入 + 输出")}</small>
              </div>
            </div>
            <div className="usage-kpi-grid">
              <div><ToneIcon type="request" /><span>{copy("Requests", "请求")}</span><strong>{aggregate.requests.toLocaleString(language)}</strong></div>
              <div><ToneIcon type="cost" /><span>{copy("Estimated cost", "估算成本")}</span><strong>{cost(aggregate.cost_micros)}</strong></div>
              <div><ToneIcon type="success" /><span>{copy("Success rate", "成功率")}</span><strong>{successRate(aggregate)}</strong></div>
              <div><ToneIcon type="latency" /><span>{copy("p95 latency", "p95 延迟")}</span><strong>{latency(aggregate.p95_latency_ms)}</strong></div>
            </div>
            <TokenRail aggregate={aggregate} />
            {aggregate.unpriced_requests > 0 && (
              <div className="usage-unpriced-note">
                {copy(
                  `Known cost covers ${aggregate.priced_requests} requests; ${aggregate.unpriced_requests} requests are unpriced.`,
                  `已知成本覆盖 ${aggregate.priced_requests} 个请求；另有 ${aggregate.unpriced_requests} 个请求未定价。`,
                )}
              </div>
            )}
          </section>

          <section className="usage-trend-panel">
            <header>
              <div>
                <div>
                  <h2>{copy("Usage trend", "使用趋势")}</h2>
                  <p>{sinceOptions.find((range) => range.value === since)?.label} · {since === "24h"
                    ? copy("Hourly", "按小时")
                    : copy("Daily", "按天")}</p>
                </div>
              </div>
              <span className="usage-dual-axis-note">{copy("Left: tokens · Right: cost", "左轴 Token · 右轴成本")}</span>
            </header>
            <div className="usage-chart-legend">
              <span><i className="tone-input" />{copy("Input", "输入")}</span>
              <span><i className="tone-output" />{copy("Output", "输出")}</span>
              <span><i className="tone-cache-write" />{copy("Cache write", "缓存写入")}</span>
              <span><i className="tone-cache-read" />{copy("Cache hit", "缓存命中")}</span>
              <span><i className="tone-cost" />{copy("Cost", "成本")}</span>
            </div>
            <UsageTrendChart groups={trend?.groups ?? []} range={since as UsageTrendRange} />
          </section>

          <section className="usage-detail-panel">
            <header>
              <div>
                <div><h2>{copy("Contribution details", "贡献明细")}</h2><p>{groupLabel} · {copy("Current filters", "当前筛选范围")}</p></div>
              </div>
              <div className="usage-segmented" role="tablist" aria-label={copy("Aggregation view", "统计视角")}>
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
                <thead><tr><th>{groupLabel}</th><th>{copy("Requests", "请求")}</th><th>{copy("Success rate", "成功率")}</th><th>Token</th><th>p95</th><th>{copy("Cost", "成本")}</th></tr></thead>
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
                          `${item.unpriced_requests} 个请求未定价`,
                        )}> +?</small>
                      )}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
              {(data?.groups.length ?? 0) === 0 && (
                <div className="usage-table-empty">{copy(
                  "No groups to display for this view.",
                  "当前视角没有可显示的分组。",
                )}</div>
              )}
            </div>
          </section>

        </>
      )}

      {selectedBudgets.length > 0 && (
        <section className="usage-budget-overview">
          <header>
            <div><div><h2>{copy("Budget status", "预算状态")}</h2><p>{copy("Alerts only; routing is unchanged", "仅提醒，不影响路由")}</p></div></div>
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
                      `剩余 ${formatBudgetAmount(budget.remaining_micros)} / 上限 ${formatBudgetAmount(budget.limit_micros)}`,
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

      <details className="usage-management">
        <summary>
          <span aria-hidden="true">＋</span>
          <div><strong>{copy("Budget and pricing", "预算与定价管理")}</strong><small>{copy(
            "Configure budget alerts and versioned model prices",
            "配置展示型预算预警和版本化模型价格",
          )}</small></div>
          <em>{copy("Management", "管理与口径")}</em>
        </summary>
        <div className="usage-management-body">
          <section className="budget-section">
            <div className="budget-title-row">
              <div><h3>{copy("Agent budget alerts", "Agent 预算预警")}</h3><p>{copy(
                "Calculated from stored receipt prices. Unknown prices are reported separately.",
                "按已落库 Receipt 的历史价格统计；未知价格单独提示。",
              )}</p></div>
              <span className="budget-observe-badge">{copy("ALERTS ONLY · ROUTING UNCHANGED", "仅提醒 · 不影响路由")}</span>
            </div>
            <div className="budget-form">
              <label className="field-label">Agent
                <select aria-label="Agent" className="select" value={agentId} onChange={(event) => { setAgentId(event.target.value); loadForm(event.target.value, budgets); }}>
                  {agents.map((agent) => <option key={agent.agent_id} value={agent.agent_id}>{agent.display_name}</option>)}
                </select>
              </label>
              <label className="field-label">{copy("Budget limit", "预算上限")}
                <input aria-label={copy("Budget limit", "预算上限")} className="input" type="number" min="0.000001" step="0.000001" value={limit} onChange={(event) => setLimit(event.target.value)} />
              </label>
              <label className="field-label">{copy("Warning threshold (%)", "预警阈值 (%)")}
                <input aria-label={copy("Warning threshold", "预警阈值")} className="input" type="number" min="1" max="100" step="1" value={warningPercent} onChange={(event) => setWarningPercent(event.target.value)} />
              </label>
              <label className="field-label">{copy("Period start (optional)", "周期开始（可选）")}
                <input aria-label={copy("Period start", "周期开始")} className="input" type="datetime-local" value={periodStart} onChange={(event) => setPeriodStart(event.target.value)} />
              </label>
              <label className="field-label">{copy("Period end (optional)", "周期结束（可选）")}
                <input aria-label={copy("Period end", "周期结束")} className="input" type="datetime-local" value={periodEnd} onChange={(event) => setPeriodEnd(event.target.value)} />
              </label>
              <label className="field-label">{copy("Expiry warning (days)", "到期前预警（天）")}
                <input aria-label={copy("Expiry warning", "到期前预警")} className="input" type="number" min="0" max="365" step="1" value={expiryWarningDays} onChange={(event) => setExpiryWarningDays(event.target.value)} />
              </label>
              <div className="budget-actions">
                <button className="btn primary" disabled={!agentId} onClick={saveBudget}>{copy("Save budget", "保存预算")}</button>
                <button className="btn danger" disabled={!hasSelectedBudget} onClick={deleteBudget}>{copy("Delete budget", "删除预算")}</button>
              </div>
            </div>
            {budgetErr && <div className="banner err">{budgetErr}</div>}
          </section>
          <PricingEditor />
        </div>
      </details>

      {hasFilters && (
        <button className="usage-clear-filters" type="button" onClick={() => { setAgentFilter(""); setUpstreamFilter(""); setModelFilter(""); }}>
          {copy("Clear all filters", "清除全部筛选")}
        </button>
      )}
    </section>
  );
}
