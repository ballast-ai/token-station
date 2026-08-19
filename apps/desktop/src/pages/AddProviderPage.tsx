import { useEffect, useMemo, useState } from "react";
import {
  addProvider,
  discoverProviderModels,
  importModelPricesForProvider,
  listPublicProviderModels,
  previewProviderEndpoints,
  type FreeOfferKind,
  type FreeProviderPresetView,
  type FreeProviderRegion,
  type ModelDiscoveryView,
  type ProviderEndpointPreview,
  type PublicProviderModelsView,
  type StateView,
} from "../api";
import { CUSTOM_ID, PROVIDER_CATALOG, type ProviderPreset } from "../catalog";
import {
  applyPublicProviderModels,
  listModelOfferings,
  searchModelOfferings,
  type ModelDeliveryClass,
} from "../modelCatalog";
import { ProviderIcon } from "../brandIcons";
import ModelPicker, { type CatalogStatus } from "../components/ModelPicker";
import PageBackButton from "../components/PageBackButton";
import { useLocalizedCopy } from "../components/LanguageProvider";
import { englishProviderName } from "../providerCopy";
import { humanizeAppError } from "../errors";
import { useErrorToast } from "../components/ErrorToast";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "../components/ui/select";

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

function deliveryClassLabel(
  deliveryClass: ModelDeliveryClass,
  copy: (english: string, chinese: string) => string,
): string {
  if (deliveryClass === "official") return copy("Official channel", "官方渠道");
  if (deliveryClass === "managed") return copy("Managed inference", "托管推理");
  if (deliveryClass === "self_hosted") return copy("Self-hosted", "自托管");
  if (deliveryClass === "aggregated") return copy("Aggregator", "聚合渠道");
  return copy("Provider", "供应商");
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
  onProviderSelected?: () => void;
  entryMode?: "provider-first" | "model-first";
}

const regularRegion = (preset: ProviderPreset): "china" | "global" =>
  preset.region === "中国" ? "china" : "global";

function searchableRegular(preset: ProviderPreset): string {
  return [
    preset.id,
    preset.label,
    englishProviderName(preset.id, preset.label),
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

function matchesCatalogSearch(searchableText: string, query: string): boolean {
  const terms = query
    .normalize("NFKC")
    .toLocaleLowerCase()
    .split(/[^\p{L}\p{N}]+/u)
    .filter(Boolean);
  if (terms.length === 0) return true;

  const normalizedText = searchableText
    .normalize("NFKC")
    .toLocaleLowerCase()
    .replace(/[^\p{L}\p{N}]+/gu, " ");
  return terms.every((term) => normalizedText.includes(term));
}

const PUBLIC_PRICE_PROVIDER_IDS: Record<string, string> = {
  gemini: "google",
  glm_cn: "zhipuai",
  glm: "zai",
  glm_coding: "zai-coding-plan",
  kimi: "moonshotai-cn",
  kimi_global: "moonshotai",
  qwen: "alibaba-cn",
  qwen_singapore: "alibaba",
  qwen_us: "alibaba",
  minimax_cn: "minimax-cn",
  minimax_global: "minimax",
};

function shouldDefaultPublicPriceImport(preset: ProviderPreset | undefined): boolean {
  return Boolean(preset && !preset.local && preset.subscription !== "Coding Plan");
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
  onProviderSelected,
  entryMode = "provider-first",
}: AddProviderPageProps) {
  const { showError, showInfo } = useErrorToast();
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
  const [providerDialect, setProviderDialect] = useState<
    "openai-compatible" | "azure-openai-v1"
  >("openai-compatible");
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
  const [modelFirstQuery, setModelFirstQuery] = useState("");
  const [importPublicPrices, setImportPublicPrices] = useState(false);
  const [publicModels, setPublicModels] = useState<PublicProviderModelsView | null>(null);
  const [publicModelsLoading, setPublicModelsLoading] = useState(false);
  const [publicModelsError, setPublicModelsError] = useState("");

  const catalogProviders = useMemo(
    () => applyPublicProviderModels(PROVIDER_CATALOG, publicModels),
    [publicModels],
  );

  const preset: ProviderPreset | null = useMemo(
    () => catalogProviders.find((item) => item.id === presetId) ?? null,
    [catalogProviders, presetId],
  );
  const isCustom = presetId === CUSTOM_ID;
  const isExisting = name.trim().length > 0 && existingNames.includes(name.trim());
  const needsKey = isCustom ? true : preset?.needsKey ?? true;
  const catalogModels = preset?.models ?? [];
  const allModels = [...new Set([...catalogModels, ...discoveredModels, ...extraModels])];
  const configuringRegular = Boolean(presetId);
  const publicPriceImportAvailable = !local && providerDialect !== "azure-openai-v1";
  const publicPriceProviderId = isCustom
    ? null
    : PUBLIC_PRICE_PROVIDER_IDS[presetId] ?? presetId;

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
    return [custom, ...catalogProviders].filter((item) => {
      if (
        item.id !== CUSTOM_ID
        && regularFilters.region !== "all"
        && regularRegion(item) !== regularFilters.region
      ) return false;
      if (!query) return true;
      return matchesCatalogSearch(searchableRegular(item), query);
    });
  }, [catalogProviders, copy, regularFilters]);

  const visibleFree = useMemo(() => {
    const query = freeFilters.query.trim().toLocaleLowerCase();
    return freePresets.filter((item) => {
      if (freeFilters.offer !== "all" && item.offer_kind !== freeFilters.offer) return false;
      if (freeFilters.region !== "all" && item.region !== freeFilters.region) return false;
      return matchesCatalogSearch(searchableFree(item), query);
    });
  }, [freeFilters, freePresets]);

  const visibleModelChoices = useMemo(() => {
    const query = modelFirstQuery.trim();
    return query
      ? searchModelOfferings(query, catalogProviders)
      : listModelOfferings(PROVIDER_CATALOG);
  }, [catalogProviders, modelFirstQuery]);

  useEffect(() => {
    if (catalogMode !== "regular" && entryMode !== "model-first") return;
    let active = true;
    setPublicModelsLoading(true);
    setPublicModelsError("");
    void listPublicProviderModels(PROVIDER_CATALOG.map((provider) => provider.id))
      .then((snapshot) => {
        if (!active) return;
        setPublicModels(snapshot);
      })
      .catch((caught) => {
        if (!active) return;
        setPublicModelsError(humanizeAppError(caught, language));
      })
      .finally(() => {
        if (active) setPublicModelsLoading(false);
      });
    return () => {
      active = false;
    };
  }, [catalogMode, entryMode, language]);

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

  const selectPreset = (id: string, preferredModel?: string) => {
    setPresetId(id);
    const selected = catalogProviders.find((item) => item.id === id);
    setName(selected?.id ?? "");
    setUrl(selected?.baseUrl ?? "");
    setLocal(selected?.local ?? false);
    setCredentialSource("store");
    setProviderDialect("openai-compatible");
    setImportPublicPrices(shouldDefaultPublicPriceImport(selected));
    setCredentialReference("");
    setPicked(selected
      ? preferredModel && selected.models.includes(preferredModel)
        ? [preferredModel]
        : [...selected.models]
      : []);
    setExtraModels([]);
    setDiscoveredModels([]);
    setDiscovery(null);
    setError("");
    onProviderSelected?.();
  };

  const leaveRegularConfig = () => {
    if (saving || discovering) return;
    setPresetId("");
    setName("");
    setUrl("");
    setKey("");
    setCredentialSource("store");
    setProviderDialect("openai-compatible");
    setImportPublicPrices(false);
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

  const changeProviderDialect = (next: typeof providerDialect) => {
    setPicked((current) => current.filter((model) => !discoveredModels.includes(model)));
    setDiscoveredModels([]);
    setDiscovery(null);
    setProviderDialect(next);
    if (next === "azure-openai-v1") setImportPublicPrices(false);
  };

  const refreshModels = async () => {
    if (providerDialect === "azure-openai-v1") {
      setDiscovery({
        models: [],
        source: "none",
        fetched_at_ms: null,
        warning: "model_catalog_azure_deployment_manual",
      });
      return;
    }
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
      showError(humanizeAppError(caught), `provider-model-discovery:${name.trim()}`);
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
      let next = await addProvider(
        name.trim(),
        url.trim(),
        picked,
        needsKey && credentialSource === "store" ? key : null,
        local && endpointPreview.loopback,
        needsKey ? credentialSource : "none",
        needsKey && credentialSource !== "store" ? credentialReference.trim() : null,
        providerDialect,
      );
      if (importPublicPrices && publicPriceImportAvailable) {
        try {
          const imported = await importModelPricesForProvider(
            name.trim(),
            publicPriceProviderId,
            picked,
          );
          next = imported.state;
          if (imported.missing_model_ids.length > 0) {
            showInfo(copy(
              `${imported.imported} public prices imported; ${imported.missing_model_ids.length} models remain unknown.`,
              `已导入 ${imported.imported} 个公开价格；${imported.missing_model_ids.length} 个模型仍为未知价格。`,
            ), `provider-price-import:${name.trim()}`);
          }
        } catch (priceError) {
          showError(copy(
            `Provider added, but public prices could not be imported: ${humanizeAppError(priceError, language)}`,
            `供应商已添加，但公开价格导入失败：${humanizeAppError(priceError, language)}`,
          ), `provider-price-import:${name.trim()}`);
        }
      }
      onAdded(
        next,
        isExisting
          ? copy(`Provider "${name.trim()}" updated`, `供应商“${name.trim()}”已更新`)
          : copy("Provider added", "供应商已添加"),
      );
    } catch (caught) {
      showError(humanizeAppError(caught), `provider-save:${name.trim()}`);
    } finally {
      setSaving(false);
    }
  };

  const disabled = saving || discovering;
  const publicCatalogCoverage = Object.keys(publicModels?.providers ?? {}).length;
  const publicCatalogStatus = publicModelsLoading
    ? copy("Syncing public catalog…", "正在同步公共目录…")
    : publicModelsError
      ? copy("Bundled snapshot · sync failed", "内置快照 · 同步失败")
      : publicModels?.source === "live"
        ? copy(
            `Public catalog synced · ${publicCatalogCoverage}/${PROVIDER_CATALOG.length} channels`,
            `公共目录已同步 · ${publicCatalogCoverage}/${PROVIDER_CATALOG.length} 个渠道`,
          )
        : publicModels?.source === "stale_cache"
          ? copy(
              `Stale public cache · ${publicCatalogCoverage}/${PROVIDER_CATALOG.length} channels`,
              `公共目录旧缓存 · ${publicCatalogCoverage}/${PROVIDER_CATALOG.length} 个渠道`,
            )
          : publicModels
            ? copy(
                `Public catalog cache · ${publicCatalogCoverage}/${PROVIDER_CATALOG.length} channels`,
                `公共目录缓存 · ${publicCatalogCoverage}/${PROVIDER_CATALOG.length} 个渠道`,
              )
            : copy("Bundled model snapshot", "内置模型快照");

  if (!configuringRegular && entryMode === "model-first") {
    return (
      <div className="page-stack add-provider-page model-first-page">
        <header className="page-title-row provider-catalog-title-row">
          <div>
            <PageBackButton onClick={onCancel} />
            <h1>{copy("Search models", "搜索模型")}</h1>
            <p>{copy(
              "Search for a model, then choose the provider that will deliver it.",
              "先搜索模型，再选择提供该模型的供应商。",
            )}</p>
          </div>
          <div className="provider-catalog-summary">
            <span className="provider-catalog-total">
              {copy(`${visibleModelChoices.length} choices`, `${visibleModelChoices.length} 个可选组合`)}
            </span>
            <small
              className={`public-catalog-status ${publicModelsError ? "error" : ""}`}
              role="status"
              title={publicModelsError || undefined}
            >
              {publicCatalogStatus}
            </small>
          </div>
        </header>

        <section className="panel model-first-catalog" aria-label={copy("Model search", "模型搜索")}>
          <label className="provider-catalog-search model-first-search">
            <span aria-hidden="true">⌕</span>
            <input
              autoFocus
              type="search"
              aria-label={copy("Search models", "搜索模型")}
              placeholder={copy("Enter a model name", "输入模型名称")}
              value={modelFirstQuery}
              onChange={(event) => setModelFirstQuery(event.target.value)}
            />
          </label>

          {visibleModelChoices.length > 0 ? (
            <div className="model-first-results" role="list" aria-label={copy("Models and providers", "模型与供应商")} data-layout="compact-three-column">
              {visibleModelChoices.map((offering) => {
                const { provider, model, upstreamModelId } = offering;
                const displayName = providerName(provider.id, provider.label);
                const invocationDiffers = model.label !== upstreamModelId;
                return (
                  <article role="listitem" key={offering.id}>
                    <button
                      type="button"
                      aria-label={`${model.label} · ${displayName}${invocationDiffers ? ` · ${upstreamModelId}` : ""}`}
                      title={`${model.label} · ${displayName}`}
                      onClick={() => selectPreset(provider.id, upstreamModelId)}
                    >
                      <span className="model-first-identity">
                        <span className="model-first-name">{model.label}</span>
                        {invocationDiffers ? (
                          <small className="model-first-upstream">
                            {copy(`Calls ${upstreamModelId}`, `调用 ID · ${upstreamModelId}`)}
                          </small>
                        ) : null}
                      </span>
                      <span className="model-first-provider">
                        <ProviderIcon id={provider.id} label={displayName} size={22} />
                        <span>
                          <small>{deliveryClassLabel(offering.deliveryClass, copy)}</small>
                          <strong>{displayName}</strong>
                        </span>
                      </span>
                    </button>
                  </article>
                );
              })}
            </div>
          ) : (
            <div className="provider-catalog-empty">
              <strong>{copy("No matching models", "没有匹配的模型")}</strong>
              <p>{copy("Try another model name.", "请尝试其他模型名称。")}</p>
            </div>
          )}
        </section>
      </div>
    );
  }

  if (!configuringRegular) {
    const filtersEmpty = catalogMode === "regular"
      ? regularFilters.query === "" && regularFilters.region === "all"
      : freeFilters.query === "" && freeFilters.offer === "all" && freeFilters.region === "all";
    return (
      <div className="page-stack add-provider-page unified-provider-page">
        <header className="page-title-row provider-catalog-title-row">
          <div>
            <PageBackButton onClick={onCancel} />
            <h1>{copy("Add provider", "添加供应商")}</h1>
            <p>{copy(
              "Choose a standard or free API from one catalog.",
              "从同一个目录选择常规 API 或免费 API，配置入口和返回逻辑保持一致。",
            )}</p>
          </div>
          <div className="provider-catalog-summary">
            <span className="provider-catalog-total">
              {copy(
                `${catalogMode === "regular" ? catalogProviders.length + 1 : freePresets.length} providers`,
                `${catalogMode === "regular" ? catalogProviders.length + 1 : freePresets.length} 家可选供应商`,
              )}
            </span>
            {catalogMode === "regular" ? (
              <small
                className={`public-catalog-status ${publicModelsError ? "error" : ""}`}
                role="status"
                title={publicModelsError || undefined}
              >
                {publicCatalogStatus}
              </small>
            ) : null}
          </div>
        </header>

        <section
          className="panel unified-provider-catalog"
          aria-label={copy("Choose a provider", "选择供应商")}
        >
          <div className="provider-catalog-toolbar">
            <div className="provider-mode-switch" aria-label={copy("API type", "API 类型")}>
              <button
                className={catalogMode === "regular" ? "active regular" : ""}
                type="button"
                aria-pressed={catalogMode === "regular"}
                onClick={() => switchCatalogMode("regular")}
              >
                {copy("Standard API", "常规 API")} <small>{catalogProviders.length + 1}</small>
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
            <div
              className="provider-catalog-grid"
              role="list"
              aria-label={copy("Standard providers", "常规供应商列表")}
              data-onboarding-target="provider-choice"
            >
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
            <div
              className="provider-catalog-grid"
              role="list"
              aria-label={copy("Free providers", "免费供应商列表")}
              data-onboarding-target="provider-choice"
            >
              {visibleFree.map((item) => (
                <article
                  className={`provider-catalog-card free offer-${item.offer_kind}`}
                  role="listitem"
                  key={item.id}
                >
                  <button
                    type="button"
                    onClick={() => {
                      onProviderSelected?.();
                      onSelectFree(item);
                    }}
                  >
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
        <div
          className="wizard-step"
          role="group"
          aria-label={copy("Provider credentials", "供应商凭据")}
          data-onboarding-target="provider-credential"
        >
          <div className="step-index">01</div>
          <div className="step-body form-grid">
            <label className="field-label">
              {copy("Name", "名称")}
              <input className="input" value={name} disabled={disabled || !isCustom} onChange={(event) => setName(event.target.value)} />
            </label>
            {isCustom && (
              <div className="field-label">
                <span>{copy("API dialect", "API 方言")}</span>
                <Select
                  value={providerDialect}
                  disabled={disabled}
                  onValueChange={(value) => changeProviderDialect(value as typeof providerDialect)}
                >
                  <SelectTrigger className="w-full" aria-label={copy("API dialect", "API 方言")}>
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent position="popper" align="start">
                    <SelectGroup>
                      <SelectItem value="openai-compatible">OpenAI-compatible</SelectItem>
                      <SelectItem value="azure-openai-v1">Azure OpenAI v1</SelectItem>
                    </SelectGroup>
                  </SelectContent>
                </Select>
              </div>
            )}
            <label className="field-label">
              Base URL
              <input className="input mono" value={url} disabled={disabled || !isCustom} onChange={(event) => setUrl(event.target.value)} />
            </label>
            {isCustom && providerDialect === "azure-openai-v1" && (
              <p className="inline-note form-span">
                {copy(
                  "The Base URL must point to the resource /openai/v1 root. Use the Azure deployment name as the model; the credential is sent only in api-key.",
                  "Base URL 必须指向资源的 /openai/v1 根路径；模型名填写 Azure deployment name，凭据只通过 api-key 发送。",
                )}
              </p>
            )}
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
                  <div className="field-label">
                    <span>{copy("Credential source", "凭据来源")}</span>
                    <Select
                      value={credentialSource}
                      disabled={disabled}
                      onValueChange={(value) => {
                        setCredentialSource(value as typeof credentialSource);
                        setKey("");
                        setCredentialReference("");
                      }}
                    >
                      <SelectTrigger
                        className="w-full"
                        aria-label={copy("Credential source", "凭据来源")}
                      >
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent position="popper" align="start">
                        <SelectGroup>
                          <SelectItem value="store">
                            {copy("Local store (default)", "本地存储（默认）")}
                          </SelectItem>
                          <SelectItem value="env">
                            {copy("Environment variable", "环境变量")}
                          </SelectItem>
                          <SelectItem value="file">
                            {copy("Credential file", "凭据文件")}
                          </SelectItem>
                        </SelectGroup>
                      </SelectContent>
                    </Select>
                  </div>
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

        <div
          className="wizard-step"
          role="group"
          aria-label={copy("Provider models", "供应商模型")}
          data-onboarding-target="provider-models"
        >
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
            <label className="field-label checkbox-label">
              <input
                type="checkbox"
                checked={importPublicPrices}
                disabled={disabled || !publicPriceImportAvailable}
                onChange={(event) => setImportPublicPrices(event.target.checked)}
              />
              <span>{copy(
                "Fill matching public prices",
                "批量填充匹配的公开价格",
              )}</span>
            </label>
            <p className="inline-note">
              {publicPriceImportAvailable
                ? copy(
                    "Uses models.dev public USD list prices as estimates. Existing manual prices are never overwritten; unmatched models remain unknown.",
                    "使用 models.dev 的公开美元标价作为估算。不会覆盖已有人工价格；未匹配模型保持未知价格。",
                  )
                : copy(
                    "Public price import is unavailable for local and Azure deployment entries.",
                    "本地模型和 Azure 部署条目不使用公开价格导入。",
                  )}
            </p>
          </div>
        </div>

        <footer className="wizard-actions">
          <button className="btn" type="button" disabled={disabled} onClick={leaveRegularConfig}>
            {copy("Back to catalog", "返回目录")}
          </button>
          <button
            className="btn primary"
            type="button"
            data-onboarding-target="provider-save"
            disabled={disabled || !endpointPreview || Boolean(endpointError)}
            onClick={() => void submit()}
          >
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
