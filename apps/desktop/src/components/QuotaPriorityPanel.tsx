import { useEffect, useId, useState } from "react";
import type { ProviderView, QuotaAccount, QuotaPlanView } from "../api";
import CompactCombobox, { type CompactComboboxOption } from "./CompactCombobox";
import { useLocalizedCopy } from "./LanguageProvider";
import { Field, FieldGroup, FieldLabel } from "./ui/field";
import { Input } from "./ui/input";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "./ui/select";
import { Separator } from "./ui/separator";

interface QuotaPriorityPanelProps {
  providers: ProviderView[];
  /** 已持久化的轮换账户(供应商+模型),按优先级顺序。面板以此为初值。 */
  accounts: QuotaAccount[];
  busy: boolean;
  applying: boolean;
  /** 保存并应用:上抛当前的完整账户列表(已过滤未选完的行),由上层落库+重启。 */
  onSave: (accounts: QuotaAccount[]) => void;
  /** 跳转到实时额度查看页。 */
  onViewUsage: () => void;
  /** 声明/清除某供应商的额度计划(用于本地估算)。 */
  onSavePlan: (
    upstream: string,
    lenMs: number,
    limit: number,
    unit: "tokens" | "requests",
  ) => void;
}

/** 额度计划的窗口预设,覆盖最常见的订阅刷新周期。 */
const WINDOW_PRESETS: { label: [string, string]; ms: number }[] = [
  { label: ["5 hours", "5 小时"], ms: 5 * 60 * 60 * 1000 },
  { label: ["1 day", "1 天"], ms: 24 * 60 * 60 * 1000 },
  { label: ["1 week", "1 周"], ms: 7 * 24 * 60 * 60 * 1000 },
];

type QuotaEntry = QuotaAccount;

/**
 * 额度优先模式的主面板:添加参与轮换的「供应商 + 模型」账户(不限数量),请求优先用
 * 「最接近刷新、且仍有余量」的账户,让每一份额度在刷新前用尽。行的先后即同额度时的
 * 调用优先级。此模式不出现关键词路由与只走本地。
 */
export default function QuotaPriorityPanel({
  providers,
  accounts,
  busy,
  applying,
  onSave,
  onViewUsage,
  onSavePlan,
}: QuotaPriorityPanelProps) {
  const { copy } = useLocalizedCopy();
  // 有序账户列表 —— 行顺序即同额度时的调用优先级。以已持久化的 accounts 为初值。
  const [entries, setEntries] = useState<QuotaEntry[]>(() =>
    accounts.map((account) => ({ upstream: account.upstream, model: account.model })),
  );
  // 持久化后(保存返回最新 state)重新同步:此时会顺带清掉未选完的行。编辑期间的
  // 本地改动不受影响,只有当落库内容真正变化时才重置。
  const accountsKey = accounts.map((account) => `${account.upstream}/${account.model}`).join(",");
  useEffect(() => {
    setEntries(accounts.map((account) => ({ upstream: account.upstream, model: account.model })));
    // eslint-disable-next-line react-hooks/exhaustive-deps -- accountsKey 概括了 accounts 的内容
  }, [accountsKey]);

  // 新行留空,不预填供应商;方便连选靠的是下拉里把「上次选的那家」置顶(见
  // providerOptions),而不是替用户先选好。
  const addEntry = () => setEntries((prev) => [...prev, { upstream: "", model: "" }]);
  const removeEntry = (index: number) =>
    setEntries((prev) => prev.filter((_, i) => i !== index));
  const updateEntry = (index: number, next: QuotaEntry) =>
    setEntries((prev) => prev.map((entry, i) => (i === index ? next : entry)));

  const toOption = (name: string): CompactComboboxOption => {
    const provider = providers.find((p) => p.name === name);
    return {
      value: name,
      label: name,
      hint: provider?.access_tier === "free" ? copy("Free", "免费") : undefined,
    };
  };

  // `preferred` 是「上次选的那家」——把它顶到选项最上面,连选同一供应商时点开就在
  // 第一个。当前行若已选(`current`),则以它为置顶项(带勾);否则用 preferred。
  const providerOptions = (current: string, preferred: string): CompactComboboxOption[] => {
    const pinned = current || preferred;
    const pinnedExists = providers.some((p) => p.name === pinned);
    return [
      ...(pinned && pinnedExists ? [toOption(pinned)] : []),
      { value: "", label: copy("Not selected", "未选择") },
      ...providers.filter((p) => p.name !== pinned).map((p) => toOption(p.name)),
    ];
  };

  const modelOptions = (upstream: string): CompactComboboxOption[] => {
    const provider = providers.find((p) => p.name === upstream);
    return [
      { value: "", label: copy("Not selected", "未选择") },
      ...(provider?.models ?? []).map((model) => ({ value: model, label: model })),
    ];
  };

  // 参与轮换的去重供应商(计划是按供应商的,与具体模型无关)。
  const planProviders = Array.from(
    new Set(entries.map((entry) => entry.upstream).filter(Boolean)),
  );

  return (
    <section
      className="panel quota-panel"
      aria-label={copy("Quota routing configuration", "额度路由配置")}
      data-onboarding-target="route-config"
    >
      <div className="panel-head split-heading">
        <div>
          <h2>{copy("Quota-first", "额度优先")}</h2>
          <p className="sub">
            {copy(
              "Add the accounts to rotate through — no limit. Requests prefer the account closest to refreshing that still has headroom, so every allowance is spent before it resets instead of going to waste.",
              "添加参与轮换的账户，数量不限。请求会优先用「最接近刷新、且仍有余量」的账户，让每一份额度都在刷新前尽量用尽，不被闲置。",
            )}
          </p>
        </div>
        <button type="button" className="btn quiet" onClick={onViewUsage}>
          {copy("Live quota", "实时额度")}
        </button>
      </div>

      <p className="quota-hint">
        <svg viewBox="0 0 16 16" aria-hidden="true">
          <circle cx="8" cy="8" r="7" />
          <path d="M8 7.2v4M8 4.9h.01" />
        </svg>
        <span>
          {copy(
            "When two accounts have similar quota left, requests follow the ",
            "当两家剩余额度相近时，将按下方",
          )}
          <strong>{copy("order below", "优先级顺序")}</strong>
          {copy(" — number 1 is tried first.", "调用，序号 1 最先使用。")}
        </span>
      </p>

      {providers.length === 0 ? (
        <p className="foot-hint">
          {copy(
            "No providers yet — add one under “Add provider” to connect your accounts.",
            "还没有供应商——先去「添加供应商」接入你的账户。",
          )}
        </p>
      ) : (
        <div className="quota-entry-grid">
          <div className="quota-entry-head">
            <span>{copy("Priority", "优先级")}</span>
            <span>{copy("Provider", "供应商")}</span>
            <span>{copy("Model", "模型")}</span>
            <span aria-hidden="true" />
          </div>
          {entries.length === 0 ? (
            <p className="quota-empty">{copy("No models added yet.", "还没有添加模型。")}</p>
          ) : (
            entries.map((entry, index) => {
              // 上次选的那家:当前行之前最近一个已选供应商,用于把它在下拉里置顶。
              const preferred =
                entries.slice(0, index).reverse().find((e) => e.upstream)?.upstream ?? "";
              return (
              <div className="quota-entry-row" key={index}>
                <span className="quota-provider-order">{index + 1}</span>
                <CompactCombobox
                  ariaLabel={copy(`Provider ${index + 1}`, `账户 ${index + 1} 供应商`)}
                  disabled={busy}
                  value={entry.upstream}
                  options={providerOptions(entry.upstream, preferred)}
                  onChange={(upstream) => updateEntry(index, { upstream, model: "" })}
                />
                <CompactCombobox
                  ariaLabel={copy(`Model ${index + 1}`, `账户 ${index + 1} 模型`)}
                  disabled={busy || !entry.upstream}
                  value={entry.model}
                  options={modelOptions(entry.upstream)}
                  onChange={(model) => updateEntry(index, { ...entry, model })}
                />
                <button
                  type="button"
                  className="quota-entry-remove"
                  aria-label={copy("Remove account", "移除账户")}
                  disabled={busy}
                  onClick={() => removeEntry(index)}
                >
                  ×
                </button>
              </div>
              );
            })
          )}
          <button type="button" className="btn quota-add-btn" disabled={busy} onClick={addEntry}>
            {copy("+ Add model", "+ 添加模型")}
          </button>
        </div>
      )}

      {planProviders.length > 0 && (
        <div className="quota-plan-section">
          <Separator className="quota-plan-separator" />
          <div className="quota-plan-head">
            <strong>{copy("Quota plans (optional)", "额度计划(可选)")}</strong>
            <span>
              {copy(
                "Declare each provider's reset window and allowance so its remaining can be estimated locally. Providers that report their own limits don't need this.",
                "为各供应商声明刷新窗口与额度上限,即可本地估算剩余;会自行上报限额的供应商无需填写。",
              )}
            </span>
          </div>
          {planProviders.map((upstream) => (
            <QuotaPlanRow
              key={upstream}
              upstream={upstream}
              plan={providers.find((p) => p.name === upstream)?.quota_plan ?? null}
              busy={busy}
              copy={copy}
              onSavePlan={onSavePlan}
            />
          ))}
        </div>
      )}

      <footer className="panel-foot route-actions">
        <button
          className="btn primary"
          type="button"
          data-onboarding-target="route-apply"
          disabled={busy || applying}
          onClick={() => onSave(entries.filter((entry) => entry.upstream && entry.model))}
        >
          {applying ? copy("Applying…", "应用中…") : copy("Save & apply", "保存并应用")}
        </button>
      </footer>
    </section>
  );
}

/** 单个供应商的额度计划编辑行:刷新窗口 + 额度上限 + 单位。上限为空即清除计划。 */
function QuotaPlanRow({
  upstream,
  plan,
  busy,
  copy,
  onSavePlan,
}: {
  upstream: string;
  plan: QuotaPlanView | null;
  busy: boolean;
  copy: (english: string, simplifiedChinese: string) => string;
  onSavePlan: (
    upstream: string,
    lenMs: number,
    limit: number,
    unit: "tokens" | "requests",
  ) => void;
}) {
  const presetOf = (lenMs: number | undefined): number =>
    lenMs && WINDOW_PRESETS.some((preset) => preset.ms === lenMs) ? lenMs : WINDOW_PRESETS[0].ms;
  const [lenMs, setLenMs] = useState(presetOf(plan?.len_ms));
  const [limit, setLimit] = useState(plan?.limit ? String(plan.limit) : "");
  const [unit, setUnit] = useState<"tokens" | "requests">(plan?.unit ?? "tokens");
  const windowId = useId();
  const limitId = useId();
  const unitId = useId();

  // 外部计划变化(保存后 state 刷新)时重新同步本地字段。
  useEffect(() => {
    setLenMs(presetOf(plan?.len_ms));
    setLimit(plan?.limit ? String(plan.limit) : "");
    setUnit(plan?.unit ?? "tokens");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [plan?.len_ms, plan?.limit, plan?.unit]);

  const commit = (nextLen: number, nextLimit: string, nextUnit: "tokens" | "requests") => {
    const parsed = Number.parseInt(nextLimit, 10);
    onSavePlan(upstream, nextLen, Number.isFinite(parsed) ? Math.max(0, parsed) : 0, nextUnit);
  };

  return (
    <div className="quota-plan-row">
      <span className="quota-plan-name" title={upstream}>{upstream}</span>
      <FieldGroup className="quota-plan-controls">
        <Field className="quota-plan-field" data-disabled={busy ? true : undefined}>
          <FieldLabel className="sr-only" htmlFor={windowId}>
            {copy(`${upstream} reset window`, `${upstream} 刷新窗口`)}
          </FieldLabel>
          <Select
            value={String(lenMs)}
            disabled={busy}
            onValueChange={(value) => {
              const next = Number(value);
              setLenMs(next);
              commit(next, limit, unit);
            }}
          >
            <SelectTrigger
              id={windowId}
              className="quota-plan-select-trigger"
              size="sm"
              aria-label={copy(`${upstream} reset window`, `${upstream} 刷新窗口`)}
            >
              <SelectValue />
            </SelectTrigger>
            <SelectContent align="end">
              <SelectGroup>
                {WINDOW_PRESETS.map((preset) => (
                  <SelectItem key={preset.ms} value={String(preset.ms)}>
                    {copy(preset.label[0], preset.label[1])}
                  </SelectItem>
                ))}
              </SelectGroup>
            </SelectContent>
          </Select>
        </Field>

        <Field className="quota-plan-field" data-disabled={busy ? true : undefined}>
          <FieldLabel className="sr-only" htmlFor={limitId}>
            {copy(`${upstream} allowance`, `${upstream} 额度上限`)}
          </FieldLabel>
          <Input
            id={limitId}
            className="quota-plan-limit-input"
            type="number"
            min={0}
            aria-label={copy(`${upstream} allowance`, `${upstream} 额度上限`)}
            placeholder={copy("Limit", "额度上限")}
            value={limit}
            disabled={busy}
            onChange={(event) => setLimit(event.target.value)}
            onBlur={() => commit(lenMs, limit, unit)}
          />
        </Field>

        <Field className="quota-plan-field" data-disabled={busy ? true : undefined}>
          <FieldLabel className="sr-only" htmlFor={unitId}>
            {copy(`${upstream} unit`, `${upstream} 单位`)}
          </FieldLabel>
          <Select
            value={unit}
            disabled={busy}
            onValueChange={(value) => {
              const next = value as "tokens" | "requests";
              setUnit(next);
              commit(lenMs, limit, next);
            }}
          >
            <SelectTrigger
              id={unitId}
              className="quota-plan-select-trigger"
              size="sm"
              aria-label={copy(`${upstream} unit`, `${upstream} 单位`)}
            >
              <SelectValue />
            </SelectTrigger>
            <SelectContent align="end">
              <SelectGroup>
                <SelectItem value="tokens">tokens</SelectItem>
                <SelectItem value="requests">requests</SelectItem>
              </SelectGroup>
            </SelectContent>
          </Select>
        </Field>
      </FieldGroup>
    </div>
  );
}
