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

interface FreeProviderConfigPageProps {
  preset: FreeProviderPresetView;
  onBack: () => void;
  onAdded: (state: StateView, message: string) => void;
}

const capabilityLabel = (state: CapabilityState) => {
  if (state === "verified") return "已验证";
  if (state === "declared") return "支持";
  if (state === "unsupported") return "不支持";
  return "待核验";
};

const contextLabel = (context: number) =>
  context >= 1_000_000 ? `${context / 1_000_000}M` : `${Math.round(context / 1024)}K`;

export default function FreeProviderConfigPage({
  preset,
  onBack,
  onAdded,
}: FreeProviderConfigPageProps) {
  const [apiKey, setApiKey] = useState("");
  const [showKey, setShowKey] = useState(false);
  const [selected, setSelected] = useState(() => preset.models.map((model) => model.id));
  const [guardConfirmed, setGuardConfirmed] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const requiresGuard = preset.overage_policy === "user_must_enable_guard";
  const selectedSet = useMemo(() => new Set(selected), [selected]);

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
      setError(`无法打开外部页面：${String(caught)}`);
    }
  };

  const submit = async () => {
    if (saving || !apiKey.trim() || selected.length === 0) return;
    if (requiresGuard && !guardConfirmed) {
      setError("请先确认已启用免费额度保护");
      return;
    }
    setSaving(true);
    setError("");
    try {
      const state = await addFreeProvider(
        preset.id,
        selected,
        apiKey.trim(),
        guardConfirmed,
      );
      setApiKey("");
      onAdded(state, `免费供应商「${preset.label}」已添加`);
    } catch (caught) {
      setError(String(caught));
    } finally {
      setSaving(false);
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
            <ProviderIcon id={preset.id} label={preset.label} size={42} />
          </span>
          <div>
            <span className="eyebrow">FREE UPSTREAM</span>
            <h1>{preset.label}</h1>
            <p><code>{preset.upstream_name}</code> · {preset.base_url}</p>
          </div>
        </div>
        <div className="free-config-badges">
          <b className={`offer-badge ${preset.offer_kind}`}>
            {preset.offer_kind === "recurring" ? "长期免费" : "试用额度"}
          </b>
          <span>{preset.region === "china" ? "中国可用" : "全球平台"}</span>
        </div>
      </header>

      <section className="free-key-instruction">
        <div>
          <span>API KEY</span>
          <strong>{preset.key_instruction}</strong>
          <small>最后核验 · {preset.verified_at}</small>
        </div>
        <div className="free-instruction-actions">
          <button
            className="btn"
            type="button"
            disabled={saving}
            onClick={() => void openExternal(preset.docs_url)}
          >
            查看文档
          </button>
          <button
            className="btn primary"
            type="button"
            disabled={saving}
            onClick={() => void openExternal(preset.application_url)}
          >
            申请免费 API Key ↗
          </button>
        </div>
      </section>

      {error && <div className="banner err">{error}</div>}

      <div className="free-config-grid">
        <section className="panel free-credential-panel">
          <div className="panel-head">
            <span className="step-index">01</span>
            <div>
              <h2>验证凭据</h2>
              <p className="sub">Key 只在验证成功后写入系统钥匙串；离开本页即从内存清除。</p>
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
                placeholder="粘贴供应商 API Key"
              />
              <button
                type="button"
                disabled={saving}
                aria-label={showKey ? "隐藏 API Key" : "显示 API Key"}
                onClick={() => setShowKey((current) => !current)}
              >
                {showKey ? "隐藏" : "显示"}
              </button>
            </span>
          </label>

          <div className="free-cost-boundary">
            <strong>零费用边界</strong>
            <p>{preset.free_note}</p>
            <span>额度耗尽后停止请求，不回退到普通付费实例。</span>
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
                我已在供应商控制台启用“仅免费额度”保护，并确认不会自动转为后付费。
              </span>
            </label>
          )}

          <button
            className="btn primary free-submit"
            type="button"
            disabled={!canSubmit}
            onClick={() => void submit()}
          >
            {saving ? "正在验证真实调用…" : "验证并添加免费供应商"}
          </button>
          <p className="free-submit-note">验证会发送一次极短真实请求，消耗少量免费额度。</p>
        </section>

        <section className="panel free-model-panel">
          <div className="panel-head free-model-head">
            <span className="step-index">02</span>
            <div>
              <h2>选择免费模型</h2>
              <p className="sub">仅可选择后端已核验的免费模型。</p>
            </div>
            <span className="selected-model-count">{selected.length} / {preset.models.length}</span>
          </div>
          <div className="free-model-actions">
            <button
              type="button"
              disabled={saving}
              onClick={() => setSelected(preset.models.map((model) => model.id))}
            >
              全选
            </button>
            <button type="button" disabled={saving} onClick={() => setSelected([])}>清空</button>
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
                  <i>工具 {capabilityLabel(model.tool)}</i>
                  <i>视觉 {capabilityLabel(model.vision)}</i>
                  <i>JSON {capabilityLabel(model.json_schema)}</i>
                  <i>{contextLabel(model.context_window)} 上下文</i>
                </span>
              </label>
            ))}
          </div>
          {selected.length === 0 && <p className="field-error">至少选择一个免费模型。</p>}
        </section>
      </div>
    </div>
  );
}
