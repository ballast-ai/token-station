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
import { useLocalizedCopy } from "../components/LanguageProvider";
import { englishProviderName } from "../providerCopy";
import { humanizeAppError } from "../errors";

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
  const { copy, language } = useLocalizedCopy();
  const offerLabel = (kind: FreeOfferKind) => (
    kind === "recurring" ? copy("Always free", "长期免费") : copy("Trial credit", "试用额度")
  );
  const providerName = (id: string, label: string) => copy(
    englishProviderName(id, label),
    label,
  );
  const [presetId, setPresetId] = useState("");
  const [name, setName] = useState("");
  const [url, setUrl] = useState("");
  const [key, setKey] = useState("");
  const [credentialSource, setCredentialSource] = useState<"store" | "env" | "file">("store");
  const [credentialReference, setCredentialReference] = useState("");
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
      label: copy("Custom configuration", "自定义配置"),
      baseUrl: "",
      models: [],
      needsKey: true,
      protocol: "openai_chat_completions",
      region: copy("Custom", "自定义"),
      subscription: copy("Manual configuration", "手动配置"),
      serviceClass: "self_hosted",
      officialDocs: "",
      modelDocs: "",
      verifiedAt: "2026-07-22",
      note: copy(
        "Enter an OpenAI-compatible endpoint and models manually.",
        "手动填写 OpenAI-compatible 地址与模型。",
      ),
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
  }, [copy, regularFilters]);

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
    setEndpointPreview(null);
    setEndpointError("");
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
        setEndpointError(humanizeAppError(caught));
      });
    return () => {
      active = false;
    };
  }, [url]);

  useEffect(() => {
    if (endpointPreview && !endpointPreview.loopback) setLocal(false);
  }, [endpointPreview]);

  const catalogWarning = discovery?.warning
    ? humanizeAppError(discovery.warning, language)
    : discovery?.warning;
  const catalogStatus: CatalogStatus = discovering
    ? { label: copy("Loading…", "正在获取…"), tone: "loading" }
    : discovery?.source === "live"
      ? {
          label: copy(
            `${discovery.models.length} models synced`,
            `已同步 ${discovery.models.length} 个`,
          ),
          tone: "live",
          warning: catalogWarning,
        }
      : discovery?.source === "cache"
        ? {
            label: copy(
              `Cached · ${discovery.models.length} models`,
              `使用缓存 · ${discovery.models.length} 个`,
            ),
            tone: "cache",
            warning: catalogWarning,
          }
        : discovery
          ? { label: copy("Failed to load", "获取失败"), tone: "error", warning: catalogWarning }
          : {
              label: copy(
                `Built-in suggestions · ${catalogModels.length}`,
                `内置建议 · ${catalogModels.length} 个`,
              ),
              tone: "idle",
            };

  const selectPreset = (id: string) => {
    setPresetId(id);
    const selected = PROVIDER_CATALOG.find((item) => item.id === id);
    setName(selected?.id ?? "");
    setUrl(selected?.baseUrl ?? "");
    setLocal(selected?.local ?? false);
    setCredentialSource("store");
    setCredentialReference("");
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
    setCredentialSource("store");
    setCredentialReference("");
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
      setDiscovery({
        models: [],
        source: "none",
        fetched_at_ms: null,
        warning: "model_catalog_provider_required",
      });
      return;
    }
    if (needsKey && credentialSource !== "store") {
      setDiscovery({
        models: [],
        source: "none",
        fetched_at_ms: null,
        warning: "model_catalog_reference_requires_save",
      });
      return;
    }
    if (needsKey && !key.trim()) {
      setDiscovery({
        models: [],
        source: "none",
        fetched_at_ms: null,
        warning: "model_catalog_api_key_required",
      });
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
    if (!presetId) return setError(copy("Select a provider first.", "请先选择供应商。"));
    if (!name.trim() || !url.trim()) {
      return setError(copy(
        "Provider name and base URL are required.",
        "供应商名称和 Base URL 不能为空。",
      ));
    }
    if (!endpointPreview || endpointError) {
      return setError(endpointError || copy(
        "The base URL is still being resolved.",
        "Base URL 尚未解析完成。",
      ));
    }
    if (picked.length === 0) {
      return setError(copy("Select at least one model.", "请至少选择一个模型。"));
    }
    if (needsKey && credentialSource === "store" && !key.trim()) {
      return setError(copy("Enter an API key.", "请填写 API Key。"));
    }
    if (needsKey && credentialSource !== "store" && !credentialReference.trim()) {
      return setError(copy(
        "Enter the environment variable name or absolute file path.",
        "请填写环境变量名或凭据文件绝对路径。",
      ));
    }
    setSaving(true);
    setError("");
    try {
      const next = await addProvider(
        name.trim(),
        url.trim(),
        picked,
        needsKey && credentialSource === "store" ? key : null,
        local && endpointPreview.loopback,
        needsKey ? credentialSource : "none",
        needsKey && credentialSource !== "store" ? credentialReference.trim() : null,
      );
      onAdded(
        next,
        isExisting
          ? copy(`Provider "${name.trim()}" updated`, `供应商“${name.trim()}”已更新`)
          : copy("Provider added", "供应商已添加"),
      );
    } catch (caught) {
      setError(humanizeAppError(caught));
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
            <h1>{copy("Add provider", "添加供应商")}</h1>
            <p>{copy(
              "Choose a standard or free API from one catalog.",
              "从同一个目录选择常规 API 或免费 API，配置入口和返回逻辑保持一致。",
            )}</p>
          </div>
          <span className="provider-catalog-total">
            {copy(
              `${catalogMode === "regular" ? PROVIDER_CATALOG.length + 1 : freePresets.length} providers`,
              `${catalogMode === "regular" ? PROVIDER_CATALOG.length + 1 : freePresets.length} 家可选供应商`,
            )}
          </span>
        </header>

        <section className="panel unified-provider-catalog">
          <div className="provider-catalog-toolbar">
            <div className="provider-mode-switch" aria-label={copy("API type", "API 类型")}>
              <button
                className={catalogMode === "regular" ? "active regular" : ""}
                type="button"
                aria-pressed={catalogMode === "regular"}
                onClick={() => switchCatalogMode("regular")}
              >
                {copy("Standard API", "常规 API")} <small>{PROVIDER_CATALOG.length + 1}</small>
              </button>
              <button
                className={catalogMode === "free" ? "active free" : ""}
                type="button"
                aria-pressed={catalogMode === "free"}
                onClick={() => switchCatalogMode("free")}
              >
                <span className="free-entry-signal" aria-hidden="true"><i /><i /><i /></span>
                {copy("Free API", "免费 API")} <small>{freePresets.length || "—"}</small>
              </button>
            </div>
            <label className="provider-catalog-search">
              <span aria-hidden="true">⌕</span>
              <input
                type="search"
                aria-label={catalogMode === "regular"
                  ? copy("Search standard providers", "搜索常规供应商")
                  : copy("Search free providers", "搜索免费供应商")}
                placeholder={copy("Search providers, models, or tags…", "搜索供应商、模型或标签…")}
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
              <div className="free-filter-row" aria-label={copy("Standard provider region", "常规供应商地区筛选")}>
                {([
                  ["all", copy("All", "全部")],
                  ["china", copy("Available in China", "中国可用")],
                  ["global", copy("Global", "全球平台")],
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
                <div className="free-filter-row" aria-label={copy("Free offer type", "免费类型筛选")}>
                  {([
                    ["all", copy("All", "全部")],
                    ["recurring", copy("Always free", "长期免费")],
                    ["trial", copy("Trial credit", "试用额度")],
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
                <div className="free-filter-row" aria-label={copy("Free provider region", "免费供应商地区筛选")}>
                  {([
                    ["all", copy("All regions", "全部地区")],
                    ["china", copy("Available in China", "中国可用")],
                    ["global", copy("Global", "全球平台")],
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
              <strong>{copy("Cost boundary", "费用边界")}</strong>
              {copy(
                "Only verified free models are saved. Requests stop when quota is exhausted and never fall back to paid instances.",
                "只保存已核验免费模型；额度耗尽时停止，不回退到付费实例。",
              )}
              <small>{copy("Last verified", "最后核验")} · {freePresets[0]?.verified_at ?? "—"}</small>
            </div>
          )}

          {catalogMode === "free" && freeLoading && (
            <div className="provider-catalog-status">{copy(
              "Loading free provider catalog…",
              "正在读取免费供应商目录…",
            )}</div>
          )}
          {catalogMode === "free" && freeError && (
            <div className="provider-catalog-status error">
              <strong>{copy("Failed to load free provider catalog", "免费供应商目录加载失败")}</strong>
              <span>{freeError}</span>
              <button className="btn" type="button" onClick={onLoadFree}>{copy("Retry", "重试")}</button>
            </div>
          )}

          {catalogMode === "regular" && visibleRegular.length > 0 && (
            <div className="provider-catalog-grid" role="list" aria-label={copy("Standard providers", "常规供应商列表")}>
              {visibleRegular.map((item) => {
                const custom = item.id === CUSTOM_ID;
                const displayName = providerName(item.id, item.label);
                return (
                  <article className="provider-catalog-card regular" role="listitem" key={item.id}>
                    <button type="button" onClick={() => selectPreset(item.id)}>
                      <span className={`provider-catalog-logo ${custom ? "custom" : ""}`}>
                        {custom ? "✎" : <ProviderIcon id={item.id} label={displayName} size={30} />}
                      </span>
                      <span className="provider-catalog-card-title">
                        <strong title={displayName}>{displayName}</strong>
                      </span>
                    </button>
                  </article>
                );
              })}
            </div>
          )}

          {catalogMode === "free" && !freeLoading && !freeError && visibleFree.length > 0 && (
            <div className="provider-catalog-grid" role="list" aria-label={copy("Free providers", "免费供应商列表")}>
              {visibleFree.map((item) => (
                <article
                  className={`provider-catalog-card free offer-${item.offer_kind}`}
                  role="listitem"
                  key={item.id}
                >
                  <button type="button" onClick={() => onSelectFree(item)}>
                    <span className="provider-catalog-logo">
                      <ProviderIcon id={item.id} label={providerName(item.id, item.label)} size={30} />
                    </span>
                    <span className="provider-catalog-card-title">
                      <strong title={providerName(item.id, item.label)}>
                        {providerName(item.id, item.label)}
                      </strong>
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
              <strong>{copy("No matching providers", "没有匹配的供应商")}</strong>
              <p>{copy(
                "No providers match the current search and filters.",
                "当前搜索词或筛选组合没有结果。",
              )}</p>
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
                  {copy("Clear filters", "清除筛选")}
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
          <h1>{preset?.label ?? copy("Custom configuration", "自定义配置")}</h1>
          <p>{copy(
            "Enter credentials and choose models to create a standard API instance.",
            "填写凭据并选择模型；保存后创建独立的常规 API 实例。",
          )}</p>
        </div>
      </header>

      {error && <div className="banner err">{error}</div>}
      {isExisting && !error && (
        <div className="banner info">
          {copy(
            `Provider "${name.trim()}" already exists. Saving updates its base URL, API key, and models instead of creating a duplicate.`,
            `供应商“${name.trim()}”已经存在。继续保存会更新它的 Base URL、API Key 和模型，不会重复创建。`,
          )}
        </div>
      )}

      <section className="panel provider-wizard">
        {preset && (
          <div className="preset-note">
            <span>{copy(
              "Review the provider documentation before saving credentials and models.",
              preset.note ?? `${preset.region} · ${preset.subscription}`,
            )}</span>
            <a href={preset.officialDocs} target="_blank" rel="noreferrer">
              {copy("Provider documentation", "官方接入文档")}
            </a>
          </div>
        )}
        <div className="wizard-step">
          <div className="step-index">01</div>
          <div className="step-body form-grid">
            <label className="field-label">
              {copy("Name", "名称")}
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
                <span>{copy(
                  "Enter a base URL to preview the final request endpoints.",
                  "填写 Base URL 后显示最终请求地址。",
                )}</span>
              )}
            </div>
            {needsKey ? (
              <div className="form-span">
                {credentialSource === "store" && (
                  <label className="field-label">
                    API Key
                    <input
                      className="input mono"
                      type="password"
                      autoComplete="off"
                      placeholder={copy("Stored in local secrets.json", "保存在本机 secrets.json")}
                      value={key}
                      disabled={disabled}
                      onChange={(event) => setKey(event.target.value)}
                    />
                  </label>
                )}
                <details className="credential-source-advanced">
                  <summary>{copy("Advanced credential source", "高级凭据来源")}</summary>
                  <label className="field-label">
                    {copy("Credential source", "凭据来源")}
                    <select
                      aria-label={copy("Credential source", "凭据来源")}
                      value={credentialSource}
                      disabled={disabled}
                      onChange={(event) => {
                        setCredentialSource(event.target.value as typeof credentialSource);
                        setKey("");
                        setCredentialReference("");
                      }}
                    >
                      <option value="store">{copy("Local store (default)", "本地存储（默认）")}</option>
                      <option value="env">{copy("Environment variable", "环境变量")}</option>
                      <option value="file">{copy("Credential file", "凭据文件")}</option>
                    </select>
                  </label>
                  {credentialSource !== "store" && (
                    <label className="field-label">
                      {credentialSource === "env"
                        ? copy("Environment variable name", "环境变量名")
                        : copy("Absolute credential file path", "凭据文件绝对路径")}
                      <input
                        className="input mono"
                        aria-label={credentialSource === "env"
                          ? copy("Environment variable name", "环境变量名")
                          : copy("Absolute credential file path", "凭据文件绝对路径")}
                        value={credentialReference}
                        disabled={disabled}
                        placeholder={credentialSource === "env" ? "DEEPSEEK_API_KEY" : "/absolute/path/provider.key"}
                        onChange={(event) => setCredentialReference(event.target.value)}
                      />
                    </label>
                  )}
                  <p className="inline-note">{copy(
                    "env/file stores only the reference. Token Station reads the value at request time.",
                    "env/file 只保存引用，Token Station 在请求时读取凭据值。",
                  )}</p>
                </details>
              </div>
            ) : (
              <div className="local-provider-note form-span">
                {copy("Local provider. No API key required.", "本地供应商，无需 API Key。")}
              </div>
            )}
            <label className="field-label form-span checkbox-label">
              <input
                type="checkbox"
                checked={local}
                disabled={disabled || !endpointPreview?.loopback}
                aria-describedby="provider-local-eligibility"
                onChange={(event) => setLocal(event.target.checked)}
              />
              <span>{copy(
                "This model runs locally (for example, Ollama or LM Studio) and can be selected by Local only routing.",
                "这是本机运行的本地模型（Ollama / LM Studio 等），可被“只走本地”路由锁定，请求不出本机。",
              )}</span>
            </label>
            <p
              id="provider-local-eligibility"
              className="inline-note form-span"
              aria-live="polite"
            >
              {!endpointPreview
                ? copy(
                    "Resolve the base URL before marking this provider as local.",
                    "Base URL 解析完成后才能判断是否为本地模型。",
                  )
                : endpointPreview.loopback
                  ? copy(
                      "A loopback endpoint was detected. This provider can be marked as local.",
                      "已检测到本机回环地址，可以标记为本地模型。",
                    )
                  : copy(
                      "Cloud endpoints cannot be marked as local. Only localhost or 127.0.0.1 endpoints qualify.",
                      "云端地址不能标记为本地模型；只有 localhost 或 127.0.0.1 等回环地址可以使用此选项。",
                    )}
            </p>
          </div>
        </div>

        <div className="wizard-step">
          <div className="step-index">02</div>
          <div className="step-body">
            <label className="field-label">{copy("Select models", "选择模型")}</label>
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
          <button className="btn" type="button" disabled={disabled} onClick={leaveRegularConfig}>
            {copy("Back to catalog", "返回目录")}
          </button>
          <button className="btn primary" type="button" disabled={disabled || !endpointPreview || Boolean(endpointError)} onClick={() => void submit()}>
            {saving
              ? copy("Saving…", "正在保存…")
              : isExisting
                ? copy("Update provider", "更新供应商")
                : copy("Add provider", "添加供应商")}
          </button>
        </footer>
      </section>
    </div>
  );
}
