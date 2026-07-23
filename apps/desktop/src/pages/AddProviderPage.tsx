import { useEffect, useMemo, useState } from "react";
import {
  addProvider,
  discoverProviderModels,
  previewProviderEndpoints,
  type ModelDiscoveryView,
  type ProviderEndpointPreview,
  type StateView,
} from "../api";
import { CUSTOM_ID, PROVIDER_CATALOG, catalogGroupLabel, type ProviderPreset } from "../catalog";
import ModelPicker, { type CatalogStatus } from "../components/ModelPicker";

const CATALOG_GROUPS = [...PROVIDER_CATALOG.reduce((groups, preset) => {
  const label = catalogGroupLabel(preset);
  const entries = groups.get(label) ?? [];
  entries.push(preset);
  groups.set(label, entries);
  return groups;
}, new Map<string, ProviderPreset[]>())];

interface AddProviderPageProps {
  existingNames: string[];
  onCancel: () => void;
  onAdded: (state: StateView, message: string) => void;
}

export default function AddProviderPage({ existingNames, onCancel, onAdded }: AddProviderPageProps) {
  const [presetId, setPresetId] = useState("");
  const [name, setName] = useState("");
  const [url, setUrl] = useState("");
  const [key, setKey] = useState("");
  const [picked, setPicked] = useState<string[]>([]);
  const [extraModels, setExtraModels] = useState<string[]>([]);
  const [discoveredModels, setDiscoveredModels] = useState<string[]>([]);
  const [discovery, setDiscovery] = useState<ModelDiscoveryView | null>(null);
  const [discovering, setDiscovering] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const [endpointPreview, setEndpointPreview] = useState<ProviderEndpointPreview | null>(null);
  const [endpointError, setEndpointError] = useState("");

  const preset: ProviderPreset | null = useMemo(
    () => PROVIDER_CATALOG.find((item) => item.id === presetId) ?? null,
    [presetId],
  );
  const isCustom = presetId === CUSTOM_ID;
  const isExisting = name.trim().length > 0 && existingNames.includes(name.trim());
  const needsKey = isCustom ? true : preset?.needsKey ?? true;
  const catalogModels = preset?.models ?? [];
  const allModels = [...new Set([...catalogModels, ...discoveredModels, ...extraModels])];

  useEffect(() => {
    const baseUrl = url.trim();
    if (!baseUrl) {
      setEndpointPreview(null);
      setEndpointError("");
      return;
    }
    let active = true;
    void previewProviderEndpoints(baseUrl)
      .then((preview) => {
        if (!active) return;
        setEndpointPreview(preview);
        setEndpointError("");
      })
      .catch((caught) => {
        if (!active) return;
        setEndpointPreview(null);
        setEndpointError(String(caught));
      });
    return () => {
      active = false;
    };
  }, [url]);

  const catalogStatus: CatalogStatus = discovering
    ? { label: "正在获取…", tone: "loading" }
    : discovery?.source === "live"
      ? { label: `已同步 ${discovery.models.length} 个`, tone: "live", warning: discovery.warning }
      : discovery?.source === "cache"
        ? { label: `使用缓存 · ${discovery.models.length} 个`, tone: "cache", warning: discovery.warning }
        : discovery
          ? { label: "获取失败", tone: "error", warning: discovery.warning }
          : { label: `内置建议 · ${catalogModels.length} 个`, tone: "idle" };

  const selectPreset = (id: string) => {
    setPresetId(id);
    const selected = PROVIDER_CATALOG.find((item) => item.id === id);
    setName(selected?.id ?? "");
    setUrl(selected?.baseUrl ?? "");
    setPicked(selected ? [...selected.models] : []);
    setExtraModels([]);
    setDiscoveredModels([]);
    setDiscovery(null);
    setError("");
  };

  const refreshModels = async () => {
    if (!name.trim() || !url.trim()) {
      setDiscovery({ models: [], source: "none", fetched_at_ms: null, warning: "请先填写供应商名称和 Base URL" });
      return;
    }
    if (needsKey && !key.trim()) {
      setDiscovery({ models: [], source: "none", fetched_at_ms: null, warning: "填写 API Key 后才能读取该供应商的模型目录" });
      return;
    }
    setDiscovering(true);
    setError("");
    try {
      const result = await discoverProviderModels(name.trim(), url.trim(), needsKey ? key : null);
      setDiscovery(result);
      setDiscoveredModels((current) => [...new Set([...current, ...result.models])]);
    } catch (caught) {
      setDiscovery({ models: [], source: "none", fetched_at_ms: null, warning: String(caught) });
    } finally {
      setDiscovering(false);
    }
  };

  const submit = async () => {
    if (!presetId) return setError("请先选择供应商");
    if (!name.trim() || !url.trim()) return setError("供应商名称和 Base URL 不能为空");
    if (!endpointPreview || endpointError) return setError(endpointError || "Base URL 尚未解析完成");
    if (picked.length === 0) return setError("请至少选择一个模型");
    setSaving(true);
    setError("");
    try {
      const next = await addProvider(name.trim(), url.trim(), picked, needsKey ? key : null);
      onAdded(next, isExisting ? `供应商「${name.trim()}」已更新` : "供应商已添加");
    } catch (caught) {
      setError(String(caught));
    } finally {
      setSaving(false);
    }
  };

  const disabled = saving || discovering;
  return (
    <div className="page-stack add-provider-page">
      <header className="page-title-row">
        <div>
          <button className="text-back" type="button" onClick={onCancel}>← 返回</button>
          <span className="eyebrow">NEW UPSTREAM</span>
          <h1>添加供应商</h1>
          <p>接入新的模型服务，并把模型加入主页和 Agent 共用的供应商目录。</p>
        </div>
      </header>

      {error && <div className="banner err">{error}</div>}
      {isExisting && !error && (
        <div className="banner info">
          供应商「{name.trim()}」已经存在。继续保存会更新它的 Base URL、API Key 和模型，不会重复创建。
        </div>
      )}
      <section className="panel provider-wizard">
        <div className="wizard-step">
          <div className="step-index">01</div>
          <div className="step-body">
            <label className="field-label" htmlFor="provider-preset">选择供应商</label>
            <select id="provider-preset" className="select" value={presetId} disabled={disabled} onChange={(event) => selectPreset(event.target.value)}>
              <option value="">— 选择供应商 —</option>
              {CATALOG_GROUPS.map(([group, entries]) => (
                <optgroup key={group} label={group}>
                  {entries.map((item) => <option key={item.id} value={item.id}>{item.label}</option>)}
                </optgroup>
              ))}
              <option value={CUSTOM_ID}>自定义…</option>
            </select>
          </div>
        </div>

        {presetId && (
          <>
            {preset && (
              <div className="preset-note">
                <span>{preset.note ?? `${preset.region} · ${preset.subscription}`}</span>
                <a href={preset.officialDocs} target="_blank" rel="noreferrer">官方接入文档</a>
              </div>
            )}
            <div className="wizard-step">
              <div className="step-index">02</div>
              <div className="step-body form-grid">
                <label className="field-label">
                  名称
                  <input className="input" value={name} disabled={disabled || !isCustom} onChange={(event) => setName(event.target.value)} />
                </label>
                <label className="field-label">
                  Base URL
                  <input className="input mono" value={url} disabled={disabled || !isCustom} onChange={(event) => setUrl(event.target.value)} />
                </label>
                <div className={`endpoint-preview form-span ${endpointError ? "invalid" : ""}`} aria-live="polite">
                  {endpointError ? (
                    <span>{endpointError}</span>
                  ) : endpointPreview ? (
                    <>
                      <span><strong>Chat</strong><code>{endpointPreview.chat}</code></span>
                      <span><strong>Responses</strong><code>{endpointPreview.responses}</code></span>
                      <span><strong>Messages</strong><code>{endpointPreview.messages}</code></span>
                    </>
                  ) : (
                    <span>填写 Base URL 后显示最终请求地址</span>
                  )}
                </div>
                {needsKey ? (
                  <label className="field-label form-span">
                    API Key
                    <input className="input mono" type="password" autoComplete="off" placeholder="只保存在系统钥匙串" value={key} disabled={disabled} onChange={(event) => setKey(event.target.value)} />
                  </label>
                ) : <div className="local-provider-note form-span">本地供应商，无需 API Key。</div>}
              </div>
            </div>

            <div className="wizard-step">
              <div className="step-index">03</div>
              <div className="step-body">
                <label className="field-label">选择模型</label>
                <ModelPicker
                  models={allModels}
                  selected={picked}
                  status={catalogStatus}
                  refreshing={discovering}
                  disabled={saving}
                  onRefresh={() => void refreshModels()}
                  onToggle={(model) => setPicked((current) => current.includes(model) ? current.filter((item) => item !== model) : [...current, model])}
                  onAdd={(model) => {
                    if (!allModels.includes(model)) setExtraModels((current) => [...current, model]);
                    if (!picked.includes(model)) setPicked((current) => [...current, model]);
                  }}
                />
              </div>
            </div>
          </>
        )}

        <footer className="wizard-actions">
          <button className="btn" type="button" disabled={disabled} onClick={onCancel}>取消</button>
          <button className="btn primary" type="button" disabled={disabled || !presetId || !endpointPreview || Boolean(endpointError)} onClick={() => void submit()}>
            {saving ? "正在保存…" : isExisting ? "更新供应商" : "添加供应商"}
          </button>
        </footer>
      </section>
    </div>
  );
}
