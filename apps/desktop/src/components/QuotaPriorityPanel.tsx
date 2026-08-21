import { useEffect, useId, useState } from "react";
import type { ProviderView, QuotaAccount, QuotaPlanView } from "../api";
import { ProviderIcon } from "../brandIcons";
import CompactCombobox, { type CompactComboboxOption } from "./CompactCombobox";
import { useLocalizedCopy, type LocalizedCopy } from "./LanguageProvider";
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
  /** Persisted rotation accounts, provider plus model, in priority order; used as initial panel state. */
  accounts: QuotaAccount[];
  busy: boolean;
  applying: boolean;
  /** Save and Apply sends the complete account list, excluding incomplete rows, for persistence and restart. */
  onSave: (accounts: QuotaAccount[]) => void;
  /** Navigate to the live quota page. */
  onViewUsage: () => void;
  /** Declare or clear a provider quota plan for local estimates. */
  onSavePlan: (
    upstream: string,
    lenMs: number,
    limit: number,
    unit: "tokens" | "requests",
  ) => void;
}

/** Quota-window presets covering common subscription reset periods. */
const WINDOW_PRESETS: { label: [string, string, string, string]; ms: number }[] = [
  { label: ["5 hours", "5 小时", "5 小時", "5 時間"], ms: 5 * 60 * 60 * 1000 },
  { label: ["1 day", "1 天", "1 天", "1 日"], ms: 24 * 60 * 60 * 1000 },
  { label: ["1 week", "1 周", "1 週", "1 週間"], ms: 7 * 24 * 60 * 60 * 1000 },
];

type QuotaEntry = QuotaAccount;

/**
 * Main quota-first panel. Add any number of provider-and-model accounts. Requests
 * prefer the account closest to reset that still has capacity, using quota before
 * it expires. Row order breaks ties. Keyword and local-only routing are hidden in this mode.
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
  // Ordered account list; row order is the tie-break priority. Initialize from persisted accounts.
  const [entries, setEntries] = useState<QuotaEntry[]>(() =>
    accounts.map((account) => ({ upstream: account.upstream, model: account.model })),
  );
  // Resynchronize after persistence returns fresh state. Preserve local edits
  // and reset only when persisted content actually changes.
  const accountsKey = accounts.map((account) => `${account.upstream}/${account.model}`).join(",");
  const validationId = useId();
  useEffect(() => {
    setEntries(accounts.map((account) => ({ upstream: account.upstream, model: account.model })));
    // eslint-disable-next-line react-hooks/exhaustive-deps -- accountsKey summarizes account content.
  }, [accountsKey]);

  // Leave new rows empty instead of preselecting a provider. providerOptions
  // makes repeated selection easy by placing the last provider first.
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
      icon: <ProviderIcon id={provider?.brand_id} label={name} size={20} />,
      hint: provider?.access_tier === "free" ? copy("Free", "免费", "免費", "無料") : undefined,
    };
  };

  // `preferred` is the last selected provider and appears first for repeated
  // selection. If the current row has a selection, put `current` first with a check.
  const providerOptions = (current: string, preferred: string): CompactComboboxOption[] => {
    const pinned = current || preferred;
    const pinnedExists = providers.some((p) => p.name === pinned);
    return [
      ...(pinned && pinnedExists ? [toOption(pinned)] : []),
      { value: "", label: copy("Not selected", "未选择", "未選擇", "選択されていません") },
      ...providers.filter((p) => p.name !== pinned).map((p) => toOption(p.name)),
    ];
  };

  const modelOptions = (upstream: string): CompactComboboxOption[] => {
    const provider = providers.find((p) => p.name === upstream);
    return [
      { value: "", label: copy("Not selected", "未选择", "未選擇", "選択されていません") },
      ...(provider?.models ?? []).map((model) => ({ value: model, label: model })),
    ];
  };

  // Deduplicated providers in the rotation; plans belong to providers, not individual models.
  const planProviders = Array.from(
    new Set(entries.map((entry) => entry.upstream).filter(Boolean)),
  );
  const hasCompleteEntry = entries.some((entry) => entry.upstream && entry.model);
  const hasIncompleteEntry = entries.some((entry) => !entry.upstream || !entry.model);
  const canApply = hasCompleteEntry && !hasIncompleteEntry;
  const applyHint = entries.length === 0
    ? copy("Add at least one provider and model before applying.", "至少添加一个供应商和模型后才能应用。", "應用前請至少新增一個供應商和模型。", "適用する前に少なくとも1つのプロバイダーとモデルを追加してください。")
    : hasIncompleteEntry
      ? copy("Complete every provider and model selection before applying.", "请完成所有账户的供应商和模型选择。", "應用前請完成所有供應商和模型的選擇。", "適用する前にすべてのプロバイダーとモデルの選択を完了してください。")
      : null;

  return (
    <section
      className="panel quota-panel"
      aria-label={copy("Quota routing configuration", "额度路由配置", "額度路由配置", "クォータルーティングの設定")}
      data-onboarding-target="route-config"
    >
      <div className="panel-head split-heading">
        <div>
          <h2>{copy("Quota-first", "额度优先", "額度優先", "クォータ優先")}</h2>
          <p className="sub">
            {copy(
              "Add the accounts to rotate through — no limit. Requests prefer the account closest to refreshing that still has headroom, so every allowance is spent before it resets instead of going to waste.",
              "添加参与轮换的账户，数量不限。请求会优先用「最接近刷新、且仍有余量」的账户，让每一份额度都在刷新前尽量用尽，不被闲置。", "新增參與輪換的帳號 — 無數量限制。請求會優先使用「最接近重新整理、且仍有餘量」的帳號，讓每一額度都在重新整理前盡量用盡，不被閒置。", "ローテーションに参加するアカウントを追加 — 無制限。リクエストは「最も近いリフレッシュ日で、まだ余裕がある」アカウントに優先されます。リフレッシュ前に各クォータをできるだけ使い切るため、無駄にされません。"
            )}
          </p>
        </div>
        <div className="quota-heading-actions">
          <button type="button" className="btn quiet" onClick={onViewUsage}>
            {copy("Live quota", "实时额度", "即時額度", "リアルタイムクォータ")}
          </button>
          <button
            className="btn primary"
            type="button"
            data-onboarding-target="route-apply"
            disabled={busy || applying || !canApply}
            onClick={() => onSave(entries)}
          >
            {applying ? copy("Applying…", "应用中…", "應用中…", "適用中…") : copy("Save & apply", "保存并应用", "儲存並應用", "保存して適用")}
          </button>
        </div>
      </div>

      <p className="quota-hint">
        <svg viewBox="0 0 16 16" aria-hidden="true">
          <circle cx="8" cy="8" r="7" />
          <path d="M8 7.2v4M8 4.9h.01" />
        </svg>
        <span>
          {copy(
            "When two accounts have similar quota left, requests follow the ",
            "当两家剩余额度相近时，将按下方", "當兩家剩餘額度相近時，請求會依照下方", "2つのアカウントの残りクォータが似ている場合、リクエストは以下の順序で"
          )}
          <strong>{copy("order below", "优先级顺序", "優先順序順序", "優先順位")}</strong>
          {copy(" — number 1 is tried first.", "调用，序号 1 最先使用。", " — 序號 1 最先使用。", " — 1番が最初に使用されます。")}
        </span>
      </p>

      {providers.length === 0 ? (
        <p className="foot-hint">
          {copy(
            "No providers yet — add one under “Add provider” to connect your accounts.",
            "还没有供应商——先去「添加供应商」接入你的账户。", "還沒有供應商 — 去「新增供應商」接入你的帳號。", "まだプロバイダーがありません — 「プロバイダーを追加」からアカウントを接続してください。"
          )}
        </p>
      ) : (
        <div className="quota-entry-grid">
          <div className="quota-entry-head">
            <span>{copy("Priority", "优先级", "優先順序", "優先順位")}</span>
            <span>{copy("Provider", "供应商", "供應商", "プロバイダー")}</span>
            <span>{copy("Model", "模型", "模型", "モデル")}</span>
            <span aria-hidden="true" />
          </div>
          {entries.length === 0 ? (
            <p className="quota-empty">{copy("No models added yet.", "还没有添加模型。", "還沒有新增模型。", "まだモデルが追加されていません。")}</p>
          ) : (
            entries.map((entry, index) => {
              // Use the nearest selected provider in an earlier row as the preferred dropdown item.
              const preferred =
                entries.slice(0, index).reverse().find((e) => e.upstream)?.upstream ?? "";
              const modelMissing = Boolean(entry.upstream && !entry.model);
              const modelErrorId = `${validationId}-model-${index}`;
              return (
              <div className="quota-entry-row quota-entry-row-top-aligned" key={index}>
                <span className="quota-provider-order">{index + 1}</span>
                <CompactCombobox
                  ariaLabel={copy(`Provider ${index + 1}`, `账户 ${index + 1} 供应商`, `帳號 ${index + 1} 供應商`, `アカウント ${index + 1} プロバイダー`)}
                  disabled={busy}
                  value={entry.upstream}
                  options={providerOptions(entry.upstream, preferred)}
                  onChange={(upstream) => updateEntry(index, { upstream, model: "" })}
                />
                <div className="grid min-w-0 gap-1">
                  <CompactCombobox
                    ariaLabel={copy(`Model ${index + 1}`, `账户 ${index + 1} 模型`, `帳號 ${index + 1} 模型`, `アカウント ${index + 1} モデル`)}
                    ariaDescribedBy={modelMissing ? modelErrorId : undefined}
                    ariaInvalid={modelMissing}
                    disabled={busy || !entry.upstream}
                    value={entry.model}
                    options={modelOptions(entry.upstream)}
                    onChange={(model) => updateEntry(index, { ...entry, model })}
                  />
                  {modelMissing && (
                    <span
                      className="text-xs text-[var(--danger)]"
                      id={modelErrorId}
                      role="status"
                    >
                      {copy("Select a model.", "请选择模型。", "請選擇模型。", "モデルを選択してください。")}
                    </span>
                  )}
                </div>
                <button
                  type="button"
                  className="quota-entry-remove"
                  aria-label={copy("Remove account", "移除账户", "移除帳號", "アカウントを削除")}
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
            {copy("+ Add model", "+ 添加模型", "+ 新增模型", "+ モデルを追加")}
          </button>
        </div>
      )}

      {planProviders.length > 0 && (
        <div className="quota-plan-section">
          <Separator className="quota-plan-separator" />
          <div className="quota-plan-head">
            <strong>{copy("Quota plans (optional)", "额度计划（可选）", "額度計畫（可選）", "クォータプラン（オプション）")}</strong>
            <div className="quota-plan-description">
              <span>{copy(
                "After you enter a provider's allowance and reset window, Token Station estimates its remaining quota locally.",
                "填写供应商的额度上限和刷新周期后，Token Station 会在本机估算剩余额度。", "當您在輸入供應商的額度上限與重新整理週期後，Token Station 會在本機估算剩餘額度。", "プロバイダーのクォータ上限とリセット周期を入力した後、Token Station はローカルで残りクォータを推定します。"
              )}</span>
              <span>{copy(
                "If the provider reports quota automatically, leave this blank.",
                "若供应商会自动上报额度，则无需填写。", "若供應商會自動上報額度，請留空。", "プロバイダーがクォータを自動的に報告する場合、この項目は空にします。"
              )}</span>
            </div>
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

      {applyHint && (
        <footer className="panel-foot route-actions">
          <p className="foot-hint" role="status" aria-live="polite">{applyHint}</p>
        </footer>
      )}
    </section>
  );
}

/** Provider quota-plan row with reset window, limit, and unit; an empty limit clears the plan. */
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
  copy: LocalizedCopy;
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

  // Resynchronize local fields when an external plan changes after saved state refreshes.
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
            {copy(`${upstream} reset window`, `${upstream} 刷新窗口`, `${upstream} 重新整理視窗`, `${upstream} リセットウィンドウ`)}
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
              aria-label={copy(`${upstream} reset window`, `${upstream} 刷新窗口`, `${upstream} 重新整理視窗`, `${upstream} リセットウィンドウ`)}
            >
              <SelectValue />
            </SelectTrigger>
            <SelectContent align="end">
              <SelectGroup>
                {WINDOW_PRESETS.map((preset) => (
                  <SelectItem key={preset.ms} value={String(preset.ms)}>
                    {copy(...preset.label)}
                  </SelectItem>
                ))}
              </SelectGroup>
            </SelectContent>
          </Select>
        </Field>

        <Field className="quota-plan-field" data-disabled={busy ? true : undefined}>
          <FieldLabel className="sr-only" htmlFor={limitId}>
            {copy(`${upstream} allowance`, `${upstream} 额度上限`, `${upstream} 額度上限`, `${upstream} クォータ上限`)}
          </FieldLabel>
          <Input
            id={limitId}
            className="quota-plan-limit-input"
            type="number"
            min={0}
            aria-label={copy(`${upstream} allowance`, `${upstream} 额度上限`, `${upstream} 額度上限`, `${upstream} クォータ上限`)}
            placeholder={copy("Limit", "额度上限", "額度上限", "クォータ上限")}
            value={limit}
            disabled={busy}
            onChange={(event) => setLimit(event.target.value)}
            onBlur={() => commit(lenMs, limit, unit)}
          />
        </Field>

        <Field className="quota-plan-field" data-disabled={busy ? true : undefined}>
          <FieldLabel className="sr-only" htmlFor={unitId}>
            {copy(`${upstream} unit`, `${upstream} 单位`, `${upstream} 單位`, `${upstream} 単位`)}
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
              aria-label={copy(`${upstream} unit`, `${upstream} 单位`, `${upstream} 單位`, `${upstream} 単位`)}
            >
              <SelectValue />
            </SelectTrigger>
            <SelectContent align="end">
              <SelectGroup>
                <SelectItem value="tokens">{copy("tokens", "Token", "Token", "トークン")}</SelectItem>
                <SelectItem value="requests">{copy("requests", "请求", "請求", "リクエスト")}</SelectItem>
              </SelectGroup>
            </SelectContent>
          </Select>
        </Field>
      </FieldGroup>
    </div>
  );
}
