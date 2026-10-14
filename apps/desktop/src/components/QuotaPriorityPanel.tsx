import { useEffect, useId, useState } from "react";
import type { ProviderView, QuotaAccount, QuotaPlanView, SaveQuotaPlan } from "../api";
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
import { Button } from "./ui/button";

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
  onSavePlan: SaveQuotaPlan;
}

/** Local estimate period presets. Provider reset alignment remains unknown. */
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
  providers: allProviders,
  accounts,
  busy,
  applying,
  onSave,
  onViewUsage,
  onSavePlan,
}: QuotaPriorityPanelProps) {
  const { copy } = useLocalizedCopy();
  const providers = allProviders.filter((provider) =>
    provider.provider !== "codex-plan" && provider.provider !== "chatgpt-plan"
    && provider.credential_source !== "codex_account" && provider.credential_source !== "chatgpt_account");
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
    new Set(entries.map((entry) => entry.upstream).filter((name) => providers.some((p) => p.name === name))),
  );
  const hasCompleteEntry = entries.some((entry) => entry.upstream && entry.model);
  const hasIncompleteEntry = entries.some((entry) => !entry.upstream || !entry.model || !providers.some((p) => p.name === entry.upstream));
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
          <Button type="button" variant="outline" onClick={onViewUsage}>
            {copy("Live quota", "实时额度", "即時額度", "リアルタイムクォータ")}
          </Button>
          <Button
            type="button"
            data-onboarding-target="route-apply"
            disabled={busy || applying || !canApply}
            onClick={() => onSave(entries)}
          >
            {applying ? copy("Applying…", "应用中…", "應用中…", "適用中…") : copy("Save & apply", "保存并应用", "儲存並應用", "保存して適用")}
          </Button>
        </div>
      </div>

      <p className="quota-hint">
        <svg viewBox="0 0 16 16" aria-hidden="true">
          <circle cx="8" cy="8" r="7" />
          <path d="M8 7.2v4M8 4.9h.01" />
        </svg>
        <span>
          {copy(
            "Accounts with quota use the earliest reset first. Within the same reset-time bucket, conversation affinity, rate headroom, and the order below break ties.",
            "有余量的账户按最早刷新时间优先。刷新时间相近时，依次考虑对话亲和、速率余量和下方序号。",
            "有餘量的帳號按最早重新整理時間優先。重新整理時間相近時，依次考慮對話親和、速率餘量和下方序號。",
            "残量のあるアカウントはリセットが早い順に使います。リセット時刻が近い場合は会話の継続性、レート余裕、下の順序で選びます。"
          )}
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
                <Button
                  type="button"
                  className="quota-entry-remove"
                  variant="ghost"
                  size="icon-sm"
                  aria-label={copy("Remove account", "移除账户", "移除帳號", "アカウントを削除")}
                  disabled={busy}
                  onClick={() => removeEntry(index)}
                >
                  ×
                </Button>
              </div>
              );
            })
          )}
          <Button type="button" className="quota-add-btn" variant="outline" disabled={busy} onClick={addEntry}>
            {copy("+ Add model", "+ 添加模型", "+ 新增模型", "+ モデルを追加")}
          </Button>
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
                "If the provider reports quota automatically, leave this blank. Local windows count only this gateway and do not identify actual provider reset times.",
                "若供应商会自动上报额度，则无需填写。本地窗口仅统计经过本网关的用量，不代表供应商的真实重置时刻。", "若供應商會自動上報額度，請留空。本機視窗僅統計經過此閘道的用量，不代表供應商的真實重置時刻。", "プロバイダーが自動報告する場合は空にします。ローカルウィンドウはこのゲートウェイの使用量のみを数え、実際のリセット時刻とは異なります。"
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

/** Explicit full-plan editor. Draft edits never save on blur. */
function QuotaPlanRow({ upstream, plan, busy, copy, onSavePlan }: {
  upstream: string;
  plan: QuotaPlanView | null;
  busy: boolean;
  copy: LocalizedCopy;
  onSavePlan: SaveQuotaPlan;
}) {
  type DraftWindow = { len: string; limit: string; custom: boolean };
  const fromPlan = (): DraftWindow[] => (plan?.windows ?? (plan ? [{ len_ms: plan.len_ms, limit: plan.limit }] : []))
    .map((window) => ({ len: String(window.len_ms), limit: String(window.limit), custom: !WINDOW_PRESETS.some((p) => p.ms === window.len_ms) }));
  const [windows, setWindows] = useState<DraftWindow[]>(fromPlan);
  const [unit, setUnit] = useState<"tokens" | "requests">(plan?.unit ?? "tokens");
  const [rate, setRate] = useState(plan?.rate_limit_per_min ? String(plan.rate_limit_per_min) : "");
  const [saving, setSaving] = useState(false);
  const [feedback, setFeedback] = useState("");
  const id = useId();
  const planKey = JSON.stringify(plan);
  useEffect(() => {
    setWindows(fromPlan());
    setUnit(plan?.unit ?? "tokens");
    setRate(plan?.rate_limit_per_min ? String(plan.rate_limit_per_min) : "");
    // eslint-disable-next-line react-hooks/exhaustive-deps -- planKey covers every persisted field.
  }, [planKey]);
  const positiveInteger = (value: string) => /^\d+$/.test(value) && Number.isSafeInteger(Number(value)) && Number(value) > 0;
  const valid = windows.every((window) => positiveInteger(window.len) && positiveInteger(window.limit))
    && new Set(windows.map((window) => Number(window.len))).size === windows.length
    && windows.length <= 16 && (rate === "" || positiveInteger(rate));
  const locked = busy || saving;
  const update = (index: number, patch: Partial<DraftWindow>) => {
    setWindows((previous) => previous.map((window, i) => i === index ? { ...window, ...patch } : window));
    setFeedback("");
  };
  const save = async () => {
    if (!valid || locked) return;
    setSaving(true);
    setFeedback("");
    try {
      const result = await onSavePlan(upstream, {
        windows: windows.map((window) => ({ len_ms: Number(window.len), limit: Number(window.limit) })),
        unit, rate_limit_per_min: rate === "" ? null : Number(rate),
      });
      setFeedback(result === false
        ? copy("Save failed. Your edits remain.", "保存失败，已保留编辑内容。", "儲存失敗，已保留編輯內容。", "保存に失敗しました。編集内容は保持されます。")
        : copy("Plan saved to draft. Save & apply to activate it.", "计划已保存到草稿，点击保存并应用后生效。", "計畫已儲存到草稿，點擊儲存並應用後生效。", "プランを下書きに保存しました。保存して適用で有効になります。"));
    } catch {
      setFeedback(copy("Save failed. Your edits remain.", "保存失败，已保留编辑内容。", "儲存失敗，已保留編輯內容。", "保存に失敗しました。編集内容は保持されます。"));
    } finally { setSaving(false); }
  };
  return (
    <div className="quota-plan-row">
      <span className="quota-plan-name" title={upstream}>{upstream}</span>
      <div className="quota-plan-editor">
        {windows.map((window, index) => {
          const prefix = copy(`${upstream} window ${index + 1}`, `${upstream} 窗口 ${index + 1}`, `${upstream} 視窗 ${index + 1}`, `${upstream} ウィンドウ ${index + 1}`);
          return <FieldGroup className="quota-plan-controls" key={index}>
            <Field className="quota-plan-field">
              <FieldLabel htmlFor={`${id}-${index}-period`}>{copy("Reset period", "刷新周期", "重新整理週期", "リセット周期")}</FieldLabel>
              <Select value={window.custom ? "custom" : window.len} disabled={locked} onValueChange={(value) => update(index, value === "custom" ? { custom: true } : { len: value, custom: false })}>
                <SelectTrigger id={`${id}-${index}-period`} size="sm" aria-label={`${prefix} ${copy("reset period", "刷新周期", "重新整理週期", "リセット周期")}`}><SelectValue /></SelectTrigger>
                <SelectContent><SelectGroup>
                  {WINDOW_PRESETS.map((preset) => <SelectItem key={preset.ms} value={String(preset.ms)}>{copy(...preset.label)}</SelectItem>)}
                  <SelectItem value="custom">{copy("Custom", "自定义", "自訂", "カスタム")}</SelectItem>
                </SelectGroup></SelectContent>
              </Select>
              {window.custom && <>
                <Input type="number" min={1} step={1} disabled={locked} value={window.len} aria-invalid={!positiveInteger(window.len)} aria-label={`${prefix} ${copy("custom period (ms)", "自定义周期（毫秒）", "自訂週期（毫秒）", "カスタム周期（ミリ秒）")}`} onChange={(event) => update(index, { len: event.target.value })} />
                <span className="text-xs text-muted-foreground">{copy("1 hour = 3,600,000 ms", "1 小时 = 3,600,000 毫秒", "1 小時 = 3,600,000 毫秒", "1時間 = 3,600,000ミリ秒")}</span>
              </>}
            </Field>
            <Field className="quota-plan-field">
              <FieldLabel htmlFor={`${id}-${index}-limit`}>{copy("Allowance", "额度上限", "額度上限", "クォータ上限")}</FieldLabel>
              <Input id={`${id}-${index}-limit`} type="number" min={1} step={1} disabled={locked} value={window.limit} aria-label={`${prefix} ${copy("allowance", "额度上限", "額度上限", "クォータ上限")}`} aria-invalid={!positiveInteger(window.limit)} onChange={(event) => update(index, { limit: event.target.value })} />
            </Field>
            <Button type="button" variant="ghost" disabled={locked} aria-label={copy(`Remove ${upstream} window ${index + 1}`, `移除 ${upstream} 窗口 ${index + 1}`, `移除 ${upstream} 視窗 ${index + 1}`, `${upstream} ウィンドウ ${index + 1} を削除`)} onClick={() => { setWindows((previous) => previous.filter((_, i) => i !== index)); setFeedback(""); }}>×</Button>
          </FieldGroup>;
        })}
        <FieldGroup className="quota-plan-controls quota-plan-shared-controls">
          <Field className="quota-plan-field">
            <FieldLabel htmlFor={`${id}-unit`}>{copy("Unit", "单位", "單位", "単位")}</FieldLabel>
            <Select value={unit} disabled={locked} onValueChange={(value) => { setUnit(value as "tokens" | "requests"); setFeedback(""); }}>
              <SelectTrigger id={`${id}-unit`} size="sm" aria-label={copy(`${upstream} unit`, `${upstream} 单位`, `${upstream} 單位`, `${upstream} 単位`)}><SelectValue /></SelectTrigger>
              <SelectContent><SelectGroup>
                <SelectItem value="tokens">{copy("tokens", "Token", "Token", "トークン")}</SelectItem>
                <SelectItem value="requests">{copy("requests", "请求", "請求", "リクエスト")}</SelectItem>
              </SelectGroup></SelectContent>
            </Select>
          </Field>
          <Field className="quota-plan-field">
            <FieldLabel htmlFor={`${id}-rate`}>{copy("Requests/min (optional)", "每分钟请求上限（可选）", "每分鐘請求上限（可選）", "毎分リクエスト上限（任意）")}</FieldLabel>
            <Input id={`${id}-rate`} type="number" min={1} step={1} disabled={locked} value={rate} placeholder={copy("No limit", "不限", "不限", "制限なし")} aria-label={`${upstream} RPM`} aria-invalid={rate !== "" && !positiveInteger(rate)} onChange={(event) => { setRate(event.target.value); setFeedback(""); }} />
          </Field>
        </FieldGroup>
        <div className="quota-plan-actions">
          <Button type="button" variant="outline" disabled={locked || windows.length >= 16} aria-label={copy(`Add ${upstream} window`, `添加 ${upstream} 窗口`, `新增 ${upstream} 視窗`, `${upstream} ウィンドウを追加`)} onClick={() => {
            const preset = WINDOW_PRESETS.find((preset) => !windows.some((window) => Number(window.len) === preset.ms));
            setWindows((previous) => [...previous, { len: String(preset?.ms ?? 3_600_000), limit: "", custom: !preset }]);
            setFeedback("");
          }}>{copy("+ Add window", "+ 添加窗口", "+ 新增視窗", "+ ウィンドウを追加")}</Button>
          <Button type="button" disabled={locked || !valid} aria-label={copy(`Save ${upstream} quota plan`, `保存 ${upstream} 额度计划`, `儲存 ${upstream} 額度計畫`, `${upstream} クォータプランを保存`)} onClick={() => void save()}>{windows.length ? copy("Save plan", "保存计划", "儲存計畫", "プランを保存") : copy("Clear plan", "清空计划", "清空計畫", "プランを消去")}</Button>
        </div>
        <p role="status" aria-live="polite" className="foot-hint">{!valid ? copy("Use positive safe integers and distinct periods. Maximum: 16 windows.", "请填写正安全整数，周期不能重复，最多 16 个窗口。", "請填寫正安全整數，週期不能重複，最多 16 個視窗。", "正の安全な整数と異なる周期を入力してください。最大16ウィンドウです。") : feedback}</p>
      </div>
    </div>
  );
}
