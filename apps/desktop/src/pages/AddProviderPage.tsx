import { useEffect, useMemo, useState } from "react";
import {
  addProvider,
  discoverProviderModels,
  previewProviderEndpoints,
  type FreeOfferKind,
  type FreeProviderPresetView,
  type FreeProviderRegion,
  type ModelDiscoveryView,
  type ProviderEndpointPreview,
  type StateView,
} from "../api";
import { CUSTOM_ID, PROVIDER_CATALOG, type ProviderPreset } from "../catalog";
import { ProviderIcon } from "../brandIcons";
import ModelPicker, { type CatalogStatus } from "../components/ModelPicker";
import PageBackButton from "../components/PageBackButton";

export type ProviderCatalogMode = "regular" | "free";

export interface RegularCatalogFilters {
  query: string;
  region: "all" | "china" | "global";
}

export interface FreeCatalogFilters {
  query: string;
  offer: "all" | FreeOfferKind;
  region: "all" | FreeProviderRegion;
}

interface AddProviderPageProps {
  existingNames: string[];
  onCancel: () => void;
  onAdded: (state: StateView, message: string) => void;
  catalogMode: ProviderCatalogMode;
  onCatalogModeChange: (mode: ProviderCatalogMode) => void;
  regularFilters: RegularCatalogFilters;
  onRegularFiltersChange: (filters: RegularCatalogFilters) => void;
  freePresets: FreeProviderPresetView[];
  freeLoading: boolean;
  freeError: string;
  freeFilters: FreeCatalogFilters;
  onFreeFiltersChange: (filters: FreeCatalogFilters) => void;
  onLoadFree: () => void;
  onSelectFree: (preset: FreeProviderPresetView) => void;
}

const offerLabel = (kind: FreeOfferKind) => kind === "recurring" ? "长期免费" : "试用额度";
const regularRegion = (preset: ProviderPreset): "china" | "global" =>
  preset.region === "中国" ? "china" : "global";

function searchableRegular(preset: ProviderPreset): string {
  return [
    preset.id,
    preset.label,
    preset.note ?? "",
    preset.region,
    preset.subscription,
    ...preset.models,
  ].join(" ").toLocaleLowerCase();
}

function searchableFree(preset: FreeProviderPresetView): string {
  return [
    preset.id,
    preset.label,
    preset.free_note,
    ...preset.tags,
    ...preset.models.flatMap((model) => [model.id, model.label]),
  ].join(" ").toLocaleLowerCase();
}

export default function AddProviderPage({
  existingNames,
  onCancel,
  onAdded,
  catalogMode,
  onCatalogModeChange,
  regularFilters,
  onRegularFiltersChange,
  freePresets,
  freeLoading,
  freeError,
  freeFilters,
  onFreeFiltersChange,
  onLoadFree,
  onSelectFree,
}: AddProviderPageProps) {
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
  const [local, setLocal] = useState(false);
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
  const configuringRegular = Boolean(presetId);

  const visibleRegular = useMemo(() => {
    const query = regularFilters.query.trim().toLocaleLowerCase();
    const custom: ProviderPreset = {
      id: CUSTOM_ID,
      label: "自定义配置",
      baseUrl: "",
      models: [],
      needsKey: true,
      protocol: "openai_chat_completions",
      region: "自定义",
      subscription: "手动配置",
      serviceClass: "self_hosted",
      officialDocs: "",
      modelDocs: "",
      verifiedAt: "2026-07-22",
      note: "手动填写 OpenAI-compatible 地址与模型。",
    };
    return [custom, ...PROVIDER_CATALOG].filter((item) => {
      if (
        item.id !== CUSTOM_ID
        && regularFilters.region !== "all"
        && regularRegion(item) !== regularFilters.region
      ) return false;
      if (!query) return true;
      return searchableRegular(item).includes(query);
    });
  }, [regularFilters]);

  const visibleFree = useMemo(() => {
    const query = freeFilters.query.trim().toLocaleLowerCase();
    return freePresets.filter((item) => {
      if (freeFilters.offer !== "all" && item.offer_kind !== freeFilters.offer) return false;
      if (freeFilters.region !== "all" && item.region !== freeFilters.region) return false;
      return !query || searchableFree(item).includes(query);
    });
  }, [freeFilters, freePresets]);

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
    setLocal(selected?.local ?? false);
    setPicked(selected ? [...selected.models] : []);
    setExtraModels([]);
    setDiscoveredModels([]);
    setDiscovery(null);
    setError("");
  };

  const leaveRegularConfig = () => {
    if (saving || discovering) return;
    setPresetId("");
    setName("");
    setUrl("");
    setKey("");
    setPicked([]);
    setEndpointPreview(null);
    setEndpointError("");
    setError("");
  };

  const switchCatalogMode = (mode: ProviderCatalogMode) => {
    onCatalogModeChange(mode);
    if (mode === "free" && freePresets.length === 0 && !freeLoading) onLoadFree();
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
      const next = await addProvider(name.trim(), url.trim(), picked, needsKey ? key : null, local);
      onAdded(next, isExisting ? `供应商「${name.trim()}」已更新` : "供应商已添加");
    } catch (caught) {
      setError(String(caught));
    } finally {
      setSaving(false);
    }
  };

  const disabled = saving || discovering;

  if (!configuringRegular) {
    const filtersEmpty = catalogMode === "regular"
      ? regularFilters.query === "" && regularFilters.region === "all"
      : freeFilters.query === "" && freeFilters.offer === "all" && freeFilters.region === "all";
    return (
      <div className="page-stack add-provider-page unified-provider-page">
        <header className="page-title-row provider-catalog-title-row">
          <div>
            <PageBackButton onClick={onCancel} />
            <span className="eyebrow">NEW UPSTREAM</span>
            <h1>添加供应商</h1>
            <p>从同一个目录选择常规 API 或免费 API，配置入口和返回逻辑保持一致。</p>
          </div>
          <span className="provider-catalog-total">
            {catalogMode === "regular" ? PROVIDER_CATALOG.length + 1 : freePresets.length} 家可选供应商
          </span>
        </header>

        <section className="panel unified-provider-catalog">
          <div className="provider-catalog-toolbar">
            <div className="provider-mode-switch" aria-label="API 类型">
              <button
                className={catalogMode === "regular" ? "active regular" : ""}
                type="button"
                aria-pressed={catalogMode === "regular"}
                onClick={() => switchCatalogMode("regular")}
              >
                常规 API <small>{PROVIDER_CATALOG.length + 1}</small>
              </button>
              <button
                className={catalogMode === "free" ? "active free" : ""}
                type="button"
                aria-pressed={catalogMode === "free"}
                onClick={() => switchCatalogMode("free")}
              >
                <span className="free-entry-signal" aria-hidden="true"><i /><i /><i /></span>
                免费 API <small>{freePresets.length || "—"}</small>
              </button>
            </div>
            <label className="provider-catalog-search">
              <span aria-hidden="true">⌕</span>
              <input
                type="search"
                aria-label={catalogMode === "regular" ? "搜索常规供应商" : "搜索免费供应商"}
                placeholder="搜索供应商、模型或标签…"
                value={catalogMode === "regular" ? regularFilters.query : freeFilters.query}
                onChange={(event) => {
                  if (catalogMode === "regular") {
                    onRegularFiltersChange({ ...regularFilters, query: event.target.value });
                  } else {
                    onFreeFiltersChange({ ...freeFilters, query: event.target.value });
                  }
                }}
              />
            </label>
          </div>

          <div className="provider-catalog-filters">
            {catalogMode === "regular" ? (
              <div className="free-filter-row" aria-label="常规供应商地区筛选">
                {([
                  ["all", "全部"],
                  ["china", "中国可用"],
                  ["global", "全球平台"],
                ] as const).map(([value, label]) => (
                  <button
                    key={value}
                    className={`filter-chip ${regularFilters.region === value ? "active" : ""}`}
                    type="button"
                    aria-pressed={regularFilters.region === value}
                    onClick={() => onRegularFiltersChange({ ...regularFilters, region: value })}
                  >
                    {label}
                  </button>
                ))}
              </div>
            ) : (
              <>
                <div className="free-filter-row" aria-label="免费类型筛选">
                  {([
                    ["all", "全部"],
                    ["recurring", "长期免费"],
                    ["trial", "试用额度"],
                  ] as const).map(([value, label]) => (
                    <button
                      key={value}
                      className={`filter-chip ${freeFilters.offer === value ? "active" : ""}`}
                      type="button"
                      aria-pressed={freeFilters.offer === value}
                      onClick={() => onFreeFiltersChange({ ...freeFilters, offer: value })}
                    >
                      {label}
                    </button>
                  ))}
                </div>
                <div className="free-filter-row" aria-label="免费供应商地区筛选">
                  {([
                    ["all", "全部地区"],
                    ["china", "中国可用"],
                    ["global", "全球平台"],
                  ] as const).map(([value, label]) => (
                    <button
                      key={value}
                      className={`filter-chip ${freeFilters.region === value ? "active" : ""}`}
                      type="button"
                      aria-pressed={freeFilters.region === value}
                      onClick={() => onFreeFiltersChange({ ...freeFilters, region: value })}
                    >
                      {label}
                    </button>
                  ))}
                </div>
              </>
            )}
          </div>

          {catalogMode === "free" && (
            <div className="provider-free-boundary">
              <strong>费用边界</strong>
              只保存已核验免费模型；额度耗尽时停止，不回退到付费实例。
              <small>最后核验 · {freePresets[0]?.verified_at ?? "—"}</small>
            </div>
          )}

          {catalogMode === "free" && freeLoading && (
            <div className="provider-catalog-status">正在读取免费供应商目录…</div>
          )}
          {catalogMode === "free" && freeError && (
            <div className="provider-catalog-status error">
              <strong>免费供应商目录加载失败</strong>
              <span>{freeError}</span>
              <button className="btn" type="button" onClick={onLoadFree}>重试</button>
            </div>
          )}

          {catalogMode === "regular" && visibleRegular.length > 0 && (
            <div className="provider-catalog-grid" role="list" aria-label="常规供应商列表">
              {visibleRegular.map((item) => {
                const custom = item.id === CUSTOM_ID;
                return (
                  <article className="provider-catalog-card regular" role="listitem" key={item.id}>
                    <button type="button" onClick={() => selectPreset(item.id)}>
                      <span className={`provider-catalog-logo ${custom ? "custom" : ""}`}>
                        {custom ? "✎" : <ProviderIcon id={item.id} label={item.label} size={30} />}
                      </span>
                      <span className="provider-catalog-card-title">
                        <strong>{item.label}</strong>
                      </span>
                    </button>
                  </article>
                );
              })}
            </div>
          )}

          {catalogMode === "free" && !freeLoading && !freeError && visibleFree.length > 0 && (
            <div className="provider-catalog-grid" role="list" aria-label="免费供应商列表">
              {visibleFree.map((item) => (
                <article
                  className={`provider-catalog-card free offer-${item.offer_kind}`}
                  role="listitem"
                  key={item.id}
                >
                  <button type="button" onClick={() => onSelectFree(item)}>
                    <span className="provider-catalog-logo">
                      <ProviderIcon id={item.id} label={item.label} size={30} />
                    </span>
                    <span className="provider-catalog-card-title">
                      <strong>{item.label}</strong>
                    </span>
                    <span className="provider-catalog-card-badge">
                      <i className={item.offer_kind === "recurring" ? "free" : "trial"}>
                        {offerLabel(item.offer_kind)}
                      </i>
                    </span>
                  </button>
                </article>
              ))}
            </div>
          )}

          {(
            (catalogMode === "regular" && visibleRegular.length === 0)
            || (
              catalogMode === "free"
              && !freeLoading
              && !freeError
              && visibleFree.length === 0
            )
          ) && (
            <div className="provider-catalog-empty">
              <strong>没有匹配的供应商</strong>
              <p>当前搜索词或筛选组合没有结果。</p>
              {!filtersEmpty && (
                <button
                  className="btn"
                  type="button"
                  onClick={() => {
                    if (catalogMode === "regular") {
                      onRegularFiltersChange({ query: "", region: "all" });
                    } else {
                      onFreeFiltersChange({ query: "", offer: "all", region: "all" });
                    }
                  }}
                >
                  清除筛选
                </button>
              )}
            </div>
          )}
        </section>
      </div>
    );
  }

  return (
    <div className="page-stack add-provider-page provider-config-page">
      <header className="page-title-row">
        <div>
          <PageBackButton onClick={leaveRegularConfig} disabled={disabled} />
          <span className="eyebrow">STANDARD UPSTREAM</span>
          <h1>{preset?.label ?? "自定义配置"}</h1>
          <p>填写凭据并选择模型；保存后创建独立的常规 API 实例。</p>
        </div>
      </header>

      {error && <div className="banner err">{error}</div>}
      {isExisting && !error && (
        <div className="banner info">
          供应商「{name.trim()}」已经存在。继续保存会更新它的 Base URL、API Key 和模型，不会重复创建。
        </div>
      )}

      <section className="panel provider-wizard">
        {preset && (
          <div className="preset-note">
            <span>{preset.note ?? `${preset.region} · ${preset.subscription}`}</span>
            <a href={preset.officialDocs} target="_blank" rel="noreferrer">官方接入文档</a>
          </div>
        )}
        <div className="wizard-step">
          <div className="step-index">01</div>
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
            <label className="field-label form-span checkbox-label">
              <input
                type="checkbox"
                checked={local}
                disabled={disabled}
                onChange={(event) => setLocal(event.target.checked)}
              />
              <span>这是本机运行的本地模型(Ollama / LM Studio 等)——可被「只走本地」路由锁定，请求不出本机</span>
            </label>
          </div>
        </div>

        <div className="wizard-step">
          <div className="step-index">02</div>
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

        <footer className="wizard-actions">
          <button className="btn" type="button" disabled={disabled} onClick={leaveRegularConfig}>返回目录</button>
          <button className="btn primary" type="button" disabled={disabled || !endpointPreview || Boolean(endpointError)} onClick={() => void submit()}>
            {saving ? "正在保存…" : isExisting ? "更新供应商" : "添加供应商"}
          </button>
        </footer>
      </section>
    </div>
  );
}
