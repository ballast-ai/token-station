import { useEffect, useState } from "react";
import {
  AgentUiMetadataView,
  BudgetStatus,
  StatsView,
  getAgentBudgets,
  getStats,
  listAgentRegistry,
  removeAgentBudget,
  setAgentBudget,
} from "../api";
import PricingEditor from "../components/PricingEditor";

const SINCE = [
  { v: "all", label: "全部" },
  { v: "24h", label: "近 24h" },
  { v: "7d", label: "近 7 天" },
];
const BY = [
  { v: "", label: "总计" },
  { v: "agent", label: "按 Agent" },
  { v: "upstream", label: "按供应商" },
  { v: "model", label: "按模型" },
  { v: "pool", label: "按档位" },
  { v: "status", label: "按状态码" },
];
const SOURCES = [
  { v: "", label: "全部来源" },
  { v: "openai-chat-completions", label: "OpenAI Chat Completions" },
  { v: "openai-responses", label: "OpenAI Responses" },
  { v: "anthropic-messages", label: "Anthropic Messages" },
  { v: "google-gemini-generate-content", label: "Google Gemini GenerateContent" },
];

function cost(micros: number | null): string {
  return micros == null ? "—" : `${(micros / 1_000_000).toFixed(4)}`;
}

function localDateTime(ms: number | null): string {
  if (ms == null) return "";
  const date = new Date(ms);
  const shifted = new Date(date.getTime() - date.getTimezoneOffset() * 60_000);
  return shifted.toISOString().slice(0, 16);
}

function budgetWarning(status: BudgetStatus, name: string): string {
  const parts: string[] = [];
  if (status.usage_level === "approaching") {
    parts.push(`${name} 已使用 ${status.usage_percent.toFixed(1)}%，接近预算上限`);
  } else if (status.usage_level === "exceeded") {
    parts.push(`${name} 已使用 ${status.usage_percent.toFixed(1)}%，已超过预算上限`);
  } else if (status.usage_level === "unknown") {
    parts.push(`${name} 有 ${status.unpriced_requests} 个请求尚无可用价格，预算用量暂不完整`);
  } else {
    parts.push(`${name} 已使用 ${status.usage_percent.toFixed(1)}%`);
  }
  if (status.expiry_level === "expiring") parts.push("预算周期即将到期");
  if (status.expiry_level === "expired") parts.push("预算周期已到期");
  return `${parts.join("；")}。仅提醒，不影响路由。`;
}

export default function Stats() {
  const [since, setSince] = useState("all");
  const [by, setBy] = useState("");
  const [agentFilter, setAgentFilter] = useState("");
  const [sourceFilter, setSourceFilter] = useState("");
  const [data, setData] = useState<StatsView | null>(null);
  const [err, setErr] = useState("");
  const [agents, setAgents] = useState<AgentUiMetadataView[]>([]);
  const [budgets, setBudgets] = useState<BudgetStatus[]>([]);
  const [budgetErr, setBudgetErr] = useState("");
  const [budgetSaved, setBudgetSaved] = useState("");
  const [agentId, setAgentId] = useState("");
  const [limit, setLimit] = useState("10");
  const [warningPercent, setWarningPercent] = useState("80");
  const [periodStart, setPeriodStart] = useState("");
  const [periodEnd, setPeriodEnd] = useState("");
  const [expiryWarningDays, setExpiryWarningDays] = useState("7");

  useEffect(() => {
    setErr("");
    getStats(since, by || null, agentFilter || null, sourceFilter || null)
      .then(setData)
      .catch((e) => setErr(String(e)));
  }, [since, by, agentFilter, sourceFilter]);

  const loadForm = (selected: string, statuses: BudgetStatus[]) => {
    const status = statuses.find((candidate) => candidate.agent_id === selected);
    setLimit(status ? String(status.limit_micros / 1_000_000) : "10");
    setWarningPercent(status ? String(status.warning_percent) : "80");
    setPeriodStart(status ? localDateTime(status.period_start_ms) : "");
    setPeriodEnd(status ? localDateTime(status.period_end_ms) : "");
    setExpiryWarningDays(status ? String(status.expiry_warning_days) : "7");
  };

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
      .catch((e) => setBudgetErr(String(e)));
  }, []);

  const saveBudget = async () => {
    setBudgetErr("");
    setBudgetSaved("");
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
      setBudgetErr("预算上限必须是大于 0、不超过 90 亿且最多 6 位小数的金额");
      return;
    }
    if (!Number.isInteger(warning) || warning < 1 || warning > 100) {
      setBudgetErr("预警阈值必须是 1–100 的整数");
      return;
    }
    if (!Number.isInteger(expiryDays) || expiryDays < 0 || expiryDays > 365) {
      setBudgetErr("到期预警天数必须是 0–365 的整数");
      return;
    }
    if ((startMs != null && !Number.isFinite(startMs)) || (endMs != null && !Number.isFinite(endMs))) {
      setBudgetErr("预算周期时间无效");
      return;
    }
    if (startMs != null && endMs != null && startMs >= endMs) {
      setBudgetErr("预算周期结束时间必须晚于开始时间");
      return;
    }
    try {
      const statuses = await setAgentBudget(
        agentId,
        limitMicros,
        warning,
        startMs,
        endMs,
        expiryDays,
      );
      setBudgets(statuses);
      loadForm(agentId, statuses);
      setBudgetSaved("预算已保存 · 仅用于展示与预警");
    } catch (e) {
      setBudgetErr(String(e));
    }
  };

  const deleteBudget = async () => {
    setBudgetErr("");
    setBudgetSaved("");
    try {
      const statuses = await removeAgentBudget(agentId);
      setBudgets(statuses);
      loadForm(agentId, statuses);
      setBudgetSaved("预算已删除");
    } catch (e) {
      setBudgetErr(String(e));
    }
  };

  const displayName = (id: string) =>
    agents.find((agent) => agent.agent_id === id)?.display_name ?? id;
  const hasSelectedBudget = budgets.some((budget) => budget.agent_id === agentId);

  return (
    <section className="panel">
      <div className="panel-head">
        <h2>用量</h2>
        <p className="sub">只读聚合本地指标库。请求元数据(延迟/token/落档),永不含 prompt 内容。</p>
      </div>

      <div className="add-row">
        <select className="select" value={since} onChange={(e) => setSince(e.target.value)}>
          {SINCE.map((s) => (
            <option key={s.v} value={s.v}>
              {s.label}
            </option>
          ))}
        </select>
        <select className="select" value={by} onChange={(e) => setBy(e.target.value)}>
          {BY.map((b) => (
            <option key={b.v} value={b.v}>
              {b.label}
            </option>
          ))}
        </select>
        <select aria-label="Agent 过滤" className="select" value={agentFilter} onChange={(e) => setAgentFilter(e.target.value)}>
          <option value="">全部 Agent</option>
          {agents.map((agent) => (
            <option key={agent.agent_id} value={agent.agent_id}>{agent.display_name}</option>
          ))}
        </select>
        <select aria-label="来源过滤" className="select" value={sourceFilter} onChange={(e) => setSourceFilter(e.target.value)}>
          {SOURCES.map((source) => (
            <option key={source.v} value={source.v}>{source.label}</option>
          ))}
        </select>
      </div>

      <div className="budget-section">
        <div className="budget-title-row">
          <div>
            <h3>Agent 预算预警</h3>
            <p>按 Receipt 已落库的历史价格统计；未知价格单独提示。所有状态均为 observe-only。</p>
          </div>
          <span className="budget-observe-badge">仅提醒 · 不影响路由</span>
        </div>

        {budgets.map((budget) => {
          const warning = budget.usage_level !== "healthy"
            || budget.expiry_level === "expiring"
            || budget.expiry_level === "expired";
          return (
            <div className={`banner ${warning ? "warn" : "info"}`} key={budget.agent_id}>
              {budgetWarning(budget, displayName(budget.agent_id))}
            </div>
          );
        })}

        <div className="budget-form">
          <label className="field-label">
            Agent
            <select
              aria-label="Agent"
              className="select"
              value={agentId}
              onChange={(event) => {
                setAgentId(event.target.value);
                loadForm(event.target.value, budgets);
              }}
            >
              {agents.map((agent) => (
                <option key={agent.agent_id} value={agent.agent_id}>{agent.display_name}</option>
              ))}
            </select>
          </label>
          <label className="field-label">
            预算上限
            <input aria-label="预算上限" className="input" type="number" min="0.000001" step="0.000001" value={limit} onChange={(event) => setLimit(event.target.value)} />
          </label>
          <label className="field-label">
            预警阈值 (%)
            <input aria-label="预警阈值" className="input" type="number" min="1" max="100" step="1" value={warningPercent} onChange={(event) => setWarningPercent(event.target.value)} />
          </label>
          <label className="field-label">
            周期开始（可选）
            <input aria-label="周期开始" className="input" type="datetime-local" value={periodStart} onChange={(event) => setPeriodStart(event.target.value)} />
          </label>
          <label className="field-label">
            周期结束（可选）
            <input aria-label="周期结束" className="input" type="datetime-local" value={periodEnd} onChange={(event) => setPeriodEnd(event.target.value)} />
          </label>
          <label className="field-label">
            到期前预警（天）
            <input aria-label="到期前预警" className="input" type="number" min="0" max="365" step="1" value={expiryWarningDays} onChange={(event) => setExpiryWarningDays(event.target.value)} />
          </label>
          <div className="budget-actions">
            <button className="btn primary" disabled={!agentId} onClick={saveBudget}>保存预算</button>
            <button className="btn danger" disabled={!hasSelectedBudget} onClick={deleteBudget}>删除预算</button>
          </div>
        </div>
        {budgetErr && <div className="banner err">{budgetErr}</div>}
        {budgetSaved && <div className="banner ok">{budgetSaved}</div>}
      </div>

      <PricingEditor />

      {err && <div className="banner err">{err}</div>}

      {data?.empty && (
        <div className="empty">
          指标库还没建。开启「设置 · 本地指标」并启动一次代理、跑过请求后,这里就有数据了。
        </div>
      )}

      {data && !data.empty && (
        <>
          <div className="stat-cards">
            <div className="stat-card">
              <div className="stat-num">{data.total.requests}</div>
              <div className="stat-lbl">请求</div>
            </div>
            <div className="stat-card">
              <div className="stat-num">
                {data.total.errors}
                <span className="stat-sub">
                  {" "}
                  ({data.total.requests ? Math.round((data.total.errors / data.total.requests) * 100) : 0}%)
                </span>
              </div>
              <div className="stat-lbl">错误</div>
            </div>
            <div className="stat-card">
              <div className="stat-num">{data.total.p50_latency_ms}<span className="stat-sub"> / {data.total.p95_latency_ms} ms</span></div>
              <div className="stat-lbl">延迟 p50 / p95</div>
            </div>
            <div className="stat-card">
              <div className="stat-num">
                {data.total.input_tokens}<span className="stat-sub"> ↓ {data.total.output_tokens} ↑</span>
              </div>
              <div className="stat-lbl">token 入 / 出</div>
            </div>
            <div className="stat-card">
              <div className="stat-num">{cost(data.total.cost_micros)}</div>
              <div className="stat-lbl">成本（已定价 {data.total.priced_requests} / 未定价 {data.total.unpriced_requests}）</div>
            </div>
          </div>

          {data.groups.length > 0 && (
            <table className="grid-table">
              <thead>
                <tr>
                  <th>{BY.find((b) => b.v === by)?.label ?? by}</th>
                  <th>请求</th>
                  <th>错误</th>
                  <th>p50</th>
                  <th>p95</th>
                  <th>token 入</th>
                  <th>token 出</th>
                  <th>成本</th>
                </tr>
              </thead>
              <tbody>
                {data.groups.map(([k, a]) => (
                  <tr key={k}>
                    <td className="mono">{k}</td>
                    <td>{a.requests}</td>
                    <td>{a.errors}</td>
                    <td>{a.p50_latency_ms}</td>
                    <td>{a.p95_latency_ms}</td>
                    <td>{a.input_tokens}</td>
                    <td>{a.output_tokens}</td>
                    <td>{cost(a.cost_micros)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </>
      )}
    </section>
  );
}
