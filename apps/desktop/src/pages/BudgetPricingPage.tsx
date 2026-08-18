import { useCallback, useEffect, useState } from "react";
import {
  getAgentBudgets,
  listAgentRegistry,
  removeAgentBudget,
  setAgentBudget,
  type AgentUiMetadataView,
  type BudgetStatus,
} from "../api";
import { useErrorToast } from "../components/ErrorToast";
import { useLocalizedCopy } from "../components/LanguageProvider";
import PageBackButton from "../components/PageBackButton";
import PricingEditor from "../components/PricingEditor";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "../components/ui/select";
import { humanizeAppError } from "../errors";

function localDateTime(ms: number | null): string {
  if (ms == null) return "";
  const date = new Date(ms);
  const shifted = new Date(date.getTime() - date.getTimezoneOffset() * 60_000);
  return shifted.toISOString().slice(0, 16);
}

export default function BudgetPricingPage({ onBack }: { onBack: () => void }) {
  const { copy } = useLocalizedCopy();
  const { showError, showSuccess } = useErrorToast();
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
      showSuccess(copy("Budget saved · Alerts only", "预算已保存 · 仅用于展示与预警"), `agent-budget-save:${agentId}`);
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

  const hasSelectedBudget = budgets.some((budget) => budget.agent_id === agentId);

  return (
    <div className="page-stack usage-management-page">
      <header className="overview-heading usage-management-heading">
        <div>
          <PageBackButton onClick={onBack} />
          <h1>{copy("Budget and pricing", "预算与定价管理")}</h1>
          <p>{copy(
            "Configure display-only budget alerts and versioned model prices.",
            "配置展示型预算预警和版本化模型价格。",
          )}</p>
        </div>
      </header>

      <section className="budget-section">
        <div className="budget-title-row">
          <div><h2>{copy("Agent budget alerts", "Agent 预算预警")}</h2><p>{copy(
            "Calculated from stored receipt prices. Unknown prices are reported separately.",
            "按已落库 Receipt 的历史价格统计；未知价格单独提示。",
          )}</p></div>
          <span className="budget-observe-badge">{copy("ALERTS ONLY · ROUTING UNCHANGED", "仅提醒 · 不影响路由")}</span>
        </div>
        <div className="budget-form">
          <div className="field-label">
            <span>Agent</span>
            <Select value={agentId} onValueChange={(selected) => { setAgentId(selected); loadForm(selected, budgets); }}>
              <SelectTrigger aria-label="Agent" className="w-full min-h-[34px]">
                <SelectValue />
              </SelectTrigger>
              <SelectContent position="popper">
                <SelectGroup>
                  {agents.map((agent) => (
                    <SelectItem key={agent.agent_id} value={agent.agent_id}>
                      {agent.display_name}
                    </SelectItem>
                  ))}
                </SelectGroup>
              </SelectContent>
            </Select>
          </div>
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
            <button className="btn primary" disabled={!agentId} onClick={() => void saveBudget()}>{copy("Save budget", "保存预算")}</button>
            <button className="btn danger" disabled={!hasSelectedBudget} onClick={() => void deleteBudget()}>{copy("Delete budget", "删除预算")}</button>
          </div>
        </div>
        {budgetErr && <div className="banner err">{budgetErr}</div>}
      </section>
      <PricingEditor />
    </div>
  );
}
