import { openUrl } from "@tauri-apps/plugin-opener";
import { useMemo, useState } from "react";
import {
  addFreeProvider,
  type CapabilityState,
  type FreeProviderPresetView,
  type StateView,
} from "../api";
import { ProviderIcon } from "../brandIcons";
import PageBackButton from "../components/PageBackButton";
import { useLocalizedCopy } from "../components/LanguageProvider";
import { englishProviderName } from "../providerCopy";
import { humanizeAppError } from "../errors";
import { useErrorToast } from "../components/ErrorToast";

interface FreeProviderConfigPageProps {
  preset: FreeProviderPresetView;
  onBack: () => void;
  onAdded: (state: StateView, message: string) => void;
  onBusyChange: (busy: boolean) => void;
}

const contextLabel = (context: number) =>
  context >= 1_000_000 ? `${context / 1_000_000}M` : `${Math.round(context / 1024)}K`;

export default function FreeProviderConfigPage({
  preset,
  onBack,
  onAdded,
  onBusyChange,
}: FreeProviderConfigPageProps) {
  const { copy } = useLocalizedCopy();
  const { showError } = useErrorToast();
  const providerName = copy(
    englishProviderName(preset.id, preset.label),
    preset.label,
    englishProviderName(preset.id, preset.label),
    englishProviderName(preset.id, preset.label),
  );
  const [apiKey, setApiKey] = useState("");
  const [showKey, setShowKey] = useState(false);
  const [selected, setSelected] = useState(() => preset.models.map((model) => model.id));
  const [guardConfirmed, setGuardConfirmed] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const requiresGuard = preset.overage_policy === "user_must_enable_guard";
  const selectedSet = useMemo(() => new Set(selected), [selected]);
  const capabilityLabel = (state: CapabilityState) => {
    if (state === "verified") return copy("Verified", "已验证", "已驗證", "確認済み");
    if (state === "declared") return copy("Supported", "支持", "支援", "対応");
    if (state === "unsupported") return copy("Unsupported", "不支持", "不支援", "非対応");
    return copy("Unverified", "待核验", "待核驗", "未確認");
  };

  const toggleModel = (model: string) => {
    setSelected((current) =>
      current.includes(model)
        ? current.filter((item) => item !== model)
        : [...current, model],
    );
  };

  const openExternal = async (url: string) => {
    setError("");
    try {
      await openUrl(url);
    } catch (caught) {
      showError(
        humanizeAppError({ code: "open_external_failed", detail: caught }),
        `free-provider-open:${url}`,
      );
    }
  };

  const submit = async () => {
    if (saving || !apiKey.trim() || selected.length === 0) return;
    if (requiresGuard && !guardConfirmed) {
      setError(copy("Confirm that free-tier protection is enabled first.", "请先确认已启用免费额度保护", "請先確認已啟用免費額度保護", "無料クォータ保護が有効になっていることを確認してください"));
      return;
    }
    setSaving(true);
    onBusyChange(true);
    setError("");
    try {
      const state = await addFreeProvider(
        preset.id,
        selected,
        apiKey.trim(),
        guardConfirmed,
      );
      setApiKey("");
      onAdded(
        state,
        copy(
          `Free provider “${providerName}” verified · Save to apply`,
          `免费供应商「${preset.label}」已验证 · 待保存应用`, `免費供應商「${providerName}」已驗證 · 待儲存應用`, `無料プロバイダー「${providerName}」が確認済み · 保存して適用`
        ),
      );
    } catch (caught) {
      showError(humanizeAppError(caught), `free-provider-save:${preset.id}`);
    } finally {
      setSaving(false);
      onBusyChange(false);
    }
  };

  const canSubmit =
    !saving
    && apiKey.trim().length > 0
    && selected.length > 0
    && (!requiresGuard || guardConfirmed);

  return (
    <div className="page-stack free-provider-config-page">
      <header className="free-config-head">
        <PageBackButton onClick={onBack} disabled={saving} />
        <div className="free-config-identity">
          <span className="free-config-logo">
            <ProviderIcon id={preset.id} label={providerName} size={42} />
          </span>
          <div>
            <h1>{providerName}</h1>
            <p><code>{preset.upstream_name}</code> · {preset.base_url}</p>
          </div>
        </div>
        <div className="free-config-badges">
          <b className={`offer-badge ${preset.offer_kind}`}>
            {preset.offer_kind === "recurring"
              ? copy("Always free", "长期免费", "永久免費", "永続無料")
              : copy("Trial credit", "试用额度", "試用額度", "トライアルクォータ")}
          </b>
          <span>{preset.region === "china" ? copy("Available in China", "中国可用", "中國可用", "中国で利用可能") : copy("Global platform", "全球平台", "全球平臺", "グローバルプラットフォーム")}</span>
        </div>
      </header>

      <section className="free-key-instruction">
        <div>
          <span>API KEY</span>
          <strong>{copy(
            "Create a key in the provider console, then paste it below.",
            preset.key_instruction,
            "請在供應商主控臺建立 Key，然後貼到下方。",
            "プロバイダーのコンソールで Key を作成し、下に貼り付けてください。",
          )}</strong>
          <small>{copy("Last verified", "最后核验", "最後核驗", "最後の認証")} · {preset.verified_at}</small>
        </div>
        <div className="free-instruction-actions">
          <button
            className="btn"
            type="button"
            disabled={saving}
            onClick={() => void openExternal(preset.docs_url)}
          >
            {copy("View docs", "查看文档", "檢視文件", "ドキュメントを確認")}
          </button>
          <button
            className="btn primary"
            type="button"
            disabled={saving}
            onClick={() => void openExternal(preset.application_url)}
          >
            {copy("Get a free API key ↗", "申请免费 API Key ↗", "申請免費 API Key ↗", "無料 API Key を申請 ↗")}
          </button>
        </div>
      </section>

      {error && <div className="banner err">{error}</div>}

      <div className="free-config-grid">
        <section
          className="panel free-credential-panel"
          role="group"
          aria-label={copy("Provider credentials", "供应商凭据", "供應商憑證", "プロバイダー資格情報")}
          data-surface="flat-color-block"
          data-onboarding-target="provider-credential"
        >
          <div className="panel-head">
            <span className="step-index">01</span>
            <div>
              <h2>{copy("Verify credentials", "验证凭据", "驗證憑據", "認証情報を確認")}</h2>
              <p className="sub">{copy(
                "The key is saved to this device only after verification and is cleared from memory when you leave.",
                "Key 只在验证成功后保存到本机；离开本页即从内存清除。", "Key 只在驗證成功後儲存到本機；離開本頁即從記憶體清除。", "Key は認証成功後にのみ本機に保存され、ページを離れるとメモリから削除されます。"
              )}</p>
            </div>
          </div>

          <label className="field">
            <span className="field-label">API Key</span>
            <span className="free-key-field">
              <input
                aria-label="API Key"
                type={showKey ? "text" : "password"}
                autoComplete="off"
                spellCheck={false}
                disabled={saving}
                value={apiKey}
                onChange={(event) => setApiKey(event.target.value)}
                placeholder={copy("Paste the provider API key", "粘贴供应商 API Key", "貼上供應商 API 金鑰", "プロバイダーAPIキーを貼り付け")}
              />
              <button
                type="button"
                disabled={saving}
                aria-label={showKey ? copy("Hide API key", "隐藏 API Key", "隱藏 API 金鑰", "APIキーを非表示") : copy("Show API key", "显示 API Key", "顯示 API 金鑰", "APIキーを表示")}
                onClick={() => setShowKey((current) => !current)}
              >
                {showKey ? copy("Hide", "隐藏", "隱藏", "非表示") : copy("Show", "显示", "顯示", "表示")}
              </button>
            </span>
          </label>

          <div className="free-cost-boundary">
            <strong>{copy("Zero-cost boundary", "零费用边界", "零費用邊界", "ゼロコスト境界")}</strong>
            <p>{copy(
              "Only the verified free allowance is used.",
              preset.free_note,
              "僅使用已驗證的免費額度。",
              "検証済みの無料クォータのみを使用します。",
            )}</p>
            <span>{copy(
              "Requests stop when the allowance is exhausted; they never fall back to a paid instance.",
              "额度耗尽后停止请求，不回退到普通付费实例。", "額度耗盡後停止請求，不會回退到一般付費執行個體。", "クォータが枯渇するとリクエストが停止し、通常の課金インスタンスには戻りません。"
            )}</span>
          </div>

          {requiresGuard && (
            <label className="free-guard-confirm">
              <input
                type="checkbox"
                checked={guardConfirmed}
                disabled={saving}
                onChange={(event) => setGuardConfirmed(event.target.checked)}
              />
              <span>
                {copy(
                  "I enabled free-tier-only protection in the provider console and confirmed that it cannot switch to postpaid billing automatically.",
                  "我已在供应商控制台启用“仅免费额度”保护，并确认不会自动转为后付费。", "我已在供應商控制台啟用『僅免費額度』保護，並確認不會自動轉為後付費。", "私はプロバイダーのコンソールで『無料クォータのみ』保護を有効にし、自動的に後払いに切り替わらないことを確認しました。"
                )}
              </span>
            </label>
          )}

          <button
            className="btn primary free-submit"
            type="button"
            data-onboarding-target="provider-save"
            disabled={!canSubmit}
            onClick={() => void submit()}
          >
            {saving
              ? copy("Verifying with a live request…", "正在验证真实调用…", "正在驗證真實呼叫…", "実際のリクエストで検証中…")
              : copy("Verify and add free provider", "验证并添加免费供应商", "驗證並新增免費供應商", "検証して無料プロバイダーを追加")}
          </button>
          <p className="free-submit-note">{copy(
            "Verification sends one short live request and uses a small amount of the free allowance.",
            "验证会发送一次极短真实请求，消耗少量免费额度。", "驗證會傳送一次極短真實請求，消耗少量免費額度。", "検証では極めて短い実際のリクエストを送信し、少量の無料クォータを使用します。"
          )}</p>
        </section>

        <section
          className="panel free-model-panel"
          role="group"
          aria-label={copy("Provider models", "供应商模型", "供應商模型", "プロバイダーのモデル")}
          data-surface="flat-color-block"
          data-onboarding-target="provider-models"
        >
          <div className="panel-head free-model-head">
            <span className="step-index">02</span>
            <div>
              <h2>{copy("Select free models", "选择免费模型", "選擇免費模型", "無料モデルを選択")}</h2>
              <p className="sub">{copy(
                "Only free models verified by the backend can be selected.",
                "仅可选择后端已核验的免费模型。", "僅可選擇後端已核驗的免費模型。", "後端で検証済みの無料モデルのみを選択できます。"
              )}</p>
            </div>
            <span className="selected-model-count">{selected.length} / {preset.models.length}</span>
          </div>
          <div className="free-model-actions">
            <button
              type="button"
              disabled={saving}
              onClick={() => setSelected(preset.models.map((model) => model.id))}
            >
              {copy("Select all", "全选", "全選", "すべて選択")}
            </button>
            <button type="button" disabled={saving} onClick={() => setSelected([])}>
              {copy("Clear", "清空", "清空", "クリア")}
            </button>
          </div>
          <div className="free-model-list">
            {preset.models.map((model) => (
              <label className="free-model-option" key={model.id}>
                <input
                  type="checkbox"
                  checked={selectedSet.has(model.id)}
                  disabled={saving}
                  onChange={() => toggleModel(model.id)}
                />
                <span className="free-model-main">
                  <strong>{model.label}</strong>
                  <code>{model.id}</code>
                </span>
                <span className="free-model-caps">
                  <i>{copy("Tools", "工具", "工具", "ツール")} {capabilityLabel(model.tool)}</i>
                  <i>{copy("Vision", "视觉", "視覺", "ビジョン")} {capabilityLabel(model.vision)}</i>
                  <i>JSON {capabilityLabel(model.json_schema)}</i>
                  <i>{contextLabel(model.context_window)} {copy("context", "上下文", "上下文", "コンテキスト")}</i>
                </span>
              </label>
            ))}
          </div>
          {selected.length === 0 && (
            <p className="field-error">{copy(
              "Select at least one free model.",
              "至少选择一个免费模型。", "請至少選擇一個免費模型。", "少なくとも1つの無料モデルを選択してください。"
            )}</p>
          )}
        </section>
      </div>
    </div>
  );
}
