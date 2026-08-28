import { useEffect, useMemo, useState } from "react";
import {
  addProvider,
  discoverProviderModels,
  getState,
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
import { useLocalizedCopy, type LocalizedCopy } from "../components/LanguageProvider";
import { englishProviderName } from "../providerCopy";
import { humanizeAppError } from "../errors";
import { useErrorToast } from "../components/ErrorToast";
import { Input } from "../components/ui/input";
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
  copy: LocalizedCopy,
): string {
  if (deliveryClass === "official") return copy("Official channel", "官方渠道", "官方渠道", "公式チャネル");
  if (deliveryClass === "managed") return copy("Managed inference", "托管推理", "管理式推理", "マネージドインフェレンス");
  if (deliveryClass === "self_hosted") return copy("Self-hosted", "自托管", "自建主機", "セルフホスティング");
  if (deliveryClass === "aggregated") return copy("Aggregator", "聚合渠道", "聚合器", "アグレゲーター");
  return copy("Provider", "供应商", "供應商", "プロバイダー");
}

interface AddProviderPageProps {
  existingNames: string[];
  onCancel: () => void;
  onAdded: (state: StateView, message: string) => void;
  onStateChanged?: (state: StateView) => void;
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

function shouldDefaultPublicPriceImport(preset: ProviderPreset | undefined): boolean {
  return Boolean(preset && !preset.local && preset.subscription !== "Coding Plan");
}

export default function AddProviderPage({
  existingNames,
  onCancel,
  onAdded,
  onStateChanged,
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
    kind === "recurring" ? copy("Always free", "长期免费", "永久免費", "永続無料") : copy("Trial credit", "试用额度", "試用額度", "トライアルクォータ")
  );
  const providerName = (id: string, label: string) => copy(
    englishProviderName(id, label),
    label,
    englishProviderName(id, label),
    englishProviderName(id, label),
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
  const publicPriceImportAvailable = !isCustom
    && !local
    && providerDialect !== "azure-openai-v1";

  const visibleRegular = useMemo(() => {
    const query = regularFilters.query.trim().toLocaleLowerCase();
    const custom: ProviderPreset = {
      id: CUSTOM_ID,
      label: copy("Custom configuration", "自定义配置", "自訂配置", "カスタム設定"),
      baseUrl: "",
      models: [],
      needsKey: true,
      protocol: "openai_chat_completions",
      region: copy("Custom", "自定义", "自訂", "カスタム"),
      subscription: copy("Manual configuration", "手动配置", "手動配置", "手動設定"),
      serviceClass: "self_hosted",
      officialDocs: "",
      modelDocs: "",
      verifiedAt: "2026-07-22",
      note: copy(
        "Enter an OpenAI-compatible endpoint and models manually.",
        "手动填写 OpenAI-compatible 地址与模型。", "手動輸入 OpenAI 相容端點與模型。", "OpenAI 互換エンドポイントとモデルを手動で入力してください。"
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
      : listModelOfferings(catalogProviders);
  }, [catalogProviders, modelFirstQuery]);

  useEffect(() => {
    if (!preset || isCustom) return;
    const available = new Set([...catalogModels, ...discoveredModels, ...extraModels]);
    setPicked((current) => {
      const next = current.filter((model) => available.has(model));
      return next.length === current.length ? current : next;
    });
  }, [catalogModels, discoveredModels, extraModels, isCustom, preset]);

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
    ? { label: copy("Loading…", "正在获取…", "載入中…", "読み込み中…"), tone: "loading" }
    : discovery?.source === "live"
      ? {
          label: copy(
            `${discovery.models.length} models synced`,
            `已同步 ${discovery.models.length} 个`, `已同步 ${discovery.models.length} 個模型`, `${discovery.models.length} 個モデルが同期されました`
          ),
          tone: "live",
          warning: catalogWarning,
        }
      : discovery?.source === "cache"
        ? {
            label: copy(
              `Cached · ${discovery.models.length} models`,
              `使用缓存 · ${discovery.models.length} 个`, `使用快取 · ${discovery.models.length} 個`, `キャッシュ使用 · ${discovery.models.length} 個`
            ),
            tone: "cache",
            warning: catalogWarning,
          }
        : discovery
          ? { label: copy("Failed to load", "获取失败", "載入失敗", "読み込み失敗"), tone: "error", warning: catalogWarning }
          : {
              label: copy(
                `Built-in suggestions · ${catalogModels.length}`,
                `内置建议 · ${catalogModels.length} 个`, `內建建議 · ${catalogModels.length} 個`, `インバンド推奨 · ${catalogModels.length} 個`
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
    setPicked(selected && preferredModel && selected.models.includes(preferredModel)
      ? [preferredModel]
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
    if (!presetId) return setError(copy("Select a provider first.", "请先选择供应商。", "請先選擇供應商。", "まずプロバイダーを選択してください。"));
    if (!name.trim() || !url.trim()) {
      return setError(copy(
        "Provider name and base URL are required.",
        "供应商名称和 Base URL 不能为空。", "供應商名稱和 Base URL 為必填項。", "プロバイダー名と Base URL は必須です。"
      ));
    }
    if (!endpointPreview || endpointError) {
      return setError(endpointError || copy(
        "The base URL is still being resolved.",
        "Base URL 尚未解析完成。", "Base URL 尚未解析完成。", "Base URL はまだ解決されていません。"
      ));
    }
    if (picked.length === 0) {
      return setError(copy("Select at least one model.", "请至少选择一个模型。", "請至少選擇一個模型。", "少なくとも1つのモデルを選択してください。"));
    }
    if (needsKey && credentialSource === "store" && !key.trim()) {
      return setError(copy("Enter an API key.", "请填写 API Key。", "輸入 API Key。", "APIキーを入力してください。"));
    }
    if (needsKey && credentialSource !== "store" && !credentialReference.trim()) {
      return setError(copy(
        "Enter the environment variable name or absolute file path.",
        "请填写环境变量名或凭据文件绝对路径。", "輸入環境變數名稱或絕對檔案路徑。", "環境変数名または絶対ファイルパスを入力してください。"
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
        providerDialect,
      );
      onAdded(
        next,
        isExisting
          ? copy(`Provider "${name.trim()}" updated`, `供应商“${name.trim()}”已更新`, `供應商 "${name.trim()}" 已更新`, `プロバイダー "${name.trim()}" が更新されました`)
          : copy("Provider added", "供应商已添加", "供應商已新增", "プロバイダーが追加されました"),
      );
      if (importPublicPrices && publicPriceImportAvailable) {
        try {
          const imported = await importModelPricesForProvider(
            name.trim(),
            picked,
          );
          onStateChanged?.(imported.state);
          if (imported.missing_model_ids.length > 0) {
            showInfo(copy(
              `${imported.imported} public prices imported; ${imported.missing_model_ids.length} models remain unknown.`,
              `已导入 ${imported.imported} 个公开价格；${imported.missing_model_ids.length} 个模型仍为未知价格。`, `已匯入 ${imported.imported} 個公開價格；${imported.missing_model_ids.length} 個模型仍為未知價格。`, `${imported.imported} 個の公開価格がインポートされました；${imported.missing_model_ids.length} 個のモデルは価格が未確認です。`
            ), `provider-price-import:${name.trim()}`);
          }
        } catch (priceError) {
          showError(copy(
            `Provider added, but public prices could not be imported: ${humanizeAppError(priceError, language)}`,
            `供应商已添加，但公开价格导入失败：${humanizeAppError(priceError, language)}`, `供應商已新增，但公開價格匯入失敗：${humanizeAppError(priceError, language)}`, `プロバイダーが追加されました。ただし、公開価格のインポートに失敗しました：${humanizeAppError(priceError, language)}`
          ), `provider-price-import:${name.trim()}`);
          try {
            onStateChanged?.(await getState());
          } catch (refreshError) {
            showError(humanizeAppError(refreshError), `provider-price-refresh:${name.trim()}`);
          }
        }
      }
    } catch (caught) {
      showError(humanizeAppError(caught), `provider-save:${name.trim()}`);
    } finally {
      setSaving(false);
    }
  };

  const disabled = saving || discovering;
  const publicCatalogCoverage = Object.keys(publicModels?.providers ?? {}).length;
  const publicCatalogStatus = publicModelsLoading
    ? copy("Syncing public catalog…", "正在同步公共目录…", "正在同步公共目錄…", "パブリックカタログを同期中…")
    : publicModelsError
      ? copy("Bundled snapshot · sync failed", "内置快照 · 同步失败", "內建快照 · 同步失敗", "バンドルされたスナップショット · 同期失敗")
      : publicModels?.source === "live"
        ? copy(
            `Public catalog synced · ${publicCatalogCoverage}/${PROVIDER_CATALOG.length} channels`,
            `公共目录已同步 · ${publicCatalogCoverage}/${PROVIDER_CATALOG.length} 个渠道`, `公共目錄已同步 · ${publicCatalogCoverage}/${PROVIDER_CATALOG.length} 個渠道`, `パブリックカタログが同期されました · ${publicCatalogCoverage}/${PROVIDER_CATALOG.length} 個のチャネル`
          )
        : publicModels?.source === "stale_cache"
          ? copy(
              `Stale public cache · ${publicCatalogCoverage}/${PROVIDER_CATALOG.length} channels`,
              `公共目录旧缓存 · ${publicCatalogCoverage}/${PROVIDER_CATALOG.length} 个渠道`, `公共目錄舊快取 · ${publicCatalogCoverage}/${PROVIDER_CATALOG.length} 個渠道`, `パブリックカタログの古いキャッシュ · ${publicCatalogCoverage}/${PROVIDER_CATALOG.length} 個のチャネル`
            )
          : publicModels
            ? copy(
                `Public catalog cache · ${publicCatalogCoverage}/${PROVIDER_CATALOG.length} channels`,
                `公共目录缓存 · ${publicCatalogCoverage}/${PROVIDER_CATALOG.length} 个渠道`, `公共目錄快取 · ${publicCatalogCoverage}/${PROVIDER_CATALOG.length} 個渠道`, `パブリックカタログのキャッシュ · ${publicCatalogCoverage}/${PROVIDER_CATALOG.length} 個のチャネル`
              )
            : copy("Bundled model snapshot", "内置模型快照", "內建模型快照", "バンドルされたモデルスナップショット");

  if (!configuringRegular && entryMode === "model-first") {
    return (
      <div className="page-stack add-provider-page model-first-page">
        <header className="page-title-row provider-catalog-title-row">
          <div>
            <PageBackButton onClick={onCancel} />
            <h1>{copy("Search models", "搜索模型", "搜尋模型", "モデルを検索")}</h1>
            <p>{copy(
              "Search for a model, then choose the provider that will deliver it.",
              "先搜索模型，再选择提供该模型的供应商。", "先搜尋模型，再選擇提供該模型的供應商。", "まずモデルを検索し、そのモデルを提供するプロバイダーを選択してください。"
            )}</p>
          </div>
          <div className="provider-catalog-summary">
            <span className="provider-catalog-total">
              {copy(`${visibleModelChoices.length} choices`, `${visibleModelChoices.length} 个可选组合`, `${visibleModelChoices.length} 個可選組合`, `${visibleModelChoices.length} 個の選択肢`)}
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

        <section className="model-first-catalog" aria-label={copy("Model search", "模型搜索", "模型搜尋", "モデル検索")}>
          <label className="provider-catalog-search model-first-search">
            <span aria-hidden="true">⌕</span>
            <Input
              autoFocus
              type="search"
              className="model-first-search-input"
              aria-label={copy("Search models", "搜索模型", "搜尋模型", "モデルを検索")}
              placeholder={copy("Enter a model name", "输入模型名称", "輸入模型名稱", "モデル名を入力")}
              value={modelFirstQuery}
              onChange={(event) => setModelFirstQuery(event.target.value)}
            />
          </label>

          {visibleModelChoices.length > 0 ? (
            <div className="model-first-results" role="list" aria-label={copy("Models and providers", "模型与供应商", "模型與供應商", "モデルとプロバイダー")} data-layout="compact-three-column">
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
                            {copy(`Calls ${upstreamModelId}`, `调用 ID · ${upstreamModelId}`, `呼叫 ${upstreamModelId}`, `呼び出し ${upstreamModelId}`)}
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
              <strong>{copy("No matching models", "没有匹配的模型", "沒有匹配的模型", "一致するモデルがありません")}</strong>
              <p>{copy("Try another model name.", "请尝试其他模型名称。", "請嘗試其他模型名稱。", "別のモデル名を試してください。")}</p>
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
            <h1>{copy("Add provider", "添加供应商", "新增供應商", "プロバイダーを追加")}</h1>
            <p>{copy(
              "Choose a standard or free API from one catalog.",
              "从同一个目录选择常规 API 或免费 API，配置入口和返回逻辑保持一致。", "從同一個目錄選擇常規 API 或免費 API，配置入口和返回邏輯保持一致。", "同じディレクトリから標準APIまたは無料APIを選択し、エントリと返却ロジックを統一してください。"
            )}</p>
          </div>
          <div className="provider-catalog-summary">
            <span className="provider-catalog-total">
              {copy(
                `${catalogMode === "regular" ? catalogProviders.length + 1 : freePresets.length} providers`,
                `${catalogMode === "regular" ? catalogProviders.length + 1 : freePresets.length} 家可选供应商`, `${catalogMode === "regular" ? catalogProviders.length + 1 : freePresets.length} 家可選供應商`, `${catalogMode === "regular" ? catalogProviders.length + 1 : freePresets.length} 個の選択可能なプロバイダー`
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
          aria-label={copy("Choose a provider", "选择供应商", "選擇供應商", "プロバイダーを選択")}
        >
          <div className="provider-catalog-toolbar">
            <div className="provider-mode-switch" aria-label={copy("API type", "API 类型", "API 型別", "API タイプ")}>
              <button
                className={catalogMode === "regular" ? "active regular" : ""}
                type="button"
                aria-pressed={catalogMode === "regular"}
                onClick={() => switchCatalogMode("regular")}
              >
                {copy("Standard API", "常规 API", "常規 API", "標準API")} <small>{catalogProviders.length + 1}</small>
              </button>
              <button
                className={catalogMode === "free" ? "active free" : ""}
                type="button"
                aria-pressed={catalogMode === "free"}
                onClick={() => switchCatalogMode("free")}
              >
                <span className="free-entry-signal" aria-hidden="true"><i /><i /><i /></span>
                {copy("Free API", "免费 API", "免費 API", "無料API")} <small>{freePresets.length || "—"}</small>
              </button>
            </div>
            <label className="provider-catalog-search">
              <span aria-hidden="true">⌕</span>
              <input
                type="search"
                aria-label={catalogMode === "regular"
                  ? copy("Search standard providers", "搜索常规供应商", "搜尋常規供應商", "標準プロバイダーを検索")
                  : copy("Search free providers", "搜索免费供应商", "搜尋免費供應商", "無料プロバイダーを検索")}
                placeholder={copy("Search providers, models, or tags…", "搜索供应商、模型或标签…", "搜尋供應商、模型或標籤…", "プロバイダー、モデル、またはタグを検索…")}
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
              <div className="free-filter-row" aria-label={copy("Standard provider region", "常规供应商地区筛选", "常規供應商地區篩選", "標準プロバイダーの地域フィルタ")}>
                {([
                  ["all", copy("All", "全部", "全部", "すべて")],
                  ["china", copy("Available in China", "中国可用", "中國可用", "中国で利用可能")],
                  ["global", copy("Global", "全球平台", "全球", "グローバル")],
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
                <div className="free-filter-row" aria-label={copy("Free offer type", "免费类型筛选", "免費型別篩選", "無料タイプフィルタ")}>
                  {([
                    ["all", copy("All", "全部", "全部", "すべて")],
                    ["recurring", copy("Always free", "长期免费", "永久免費", "永続無料")],
                    ["trial", copy("Trial credit", "试用额度", "試用額度", "トライアルクォータ")],
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
                <div className="free-filter-row" aria-label={copy("Free provider region", "免费供应商地区筛选", "免費供應商地區篩選", "無料プロバイダー地域フィルタ")}>
                  {([
                    ["all", copy("All regions", "全部地区", "全部地區", "すべての地域")],
                    ["china", copy("Available in China", "中国可用", "中國可用", "中国で利用可能")],
                    ["global", copy("Global", "全球平台", "全球", "グローバル")],
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
              <strong>{copy("Cost boundary", "费用边界", "費用邊界", "費用境界")}</strong>
              {copy(
                "Only verified free models are saved. Requests stop when quota is exhausted and never fall back to paid instances.",
                "只保存已核验免费模型；额度耗尽时停止，不回退到付费实例。", "只儲存已核驗免費模型；額度耗盡時停止，不回退到付費執行個體。", "認証済みの無料モデルのみ保存されます。クォータが尽きると停止し、有料インスタンスには戻りません。"
              )}
              <small>{copy("Last verified", "最后核验", "最後核驗", "最後の認証")} · {freePresets[0]?.verified_at ?? "—"}</small>
            </div>
          )}

          {catalogMode === "free" && freeLoading && (
            <div className="provider-catalog-status">{copy(
              "Loading free provider catalog…",
              "正在读取免费供应商目录…", "正在讀取免費供應商目錄…", "無料プロバイダーのカタログを読み込んでいます…"
            )}</div>
          )}
          {catalogMode === "free" && freeError && (
            <div className="provider-catalog-status error">
              <strong>{copy("Failed to load free provider catalog", "免费供应商目录加载失败", "免費供應商目錄讀取失敗", "無料プロバイダーのカタログ読み込み失敗")}</strong>
              <span>{freeError}</span>
              <button className="btn" type="button" onClick={onLoadFree}>{copy("Retry", "重试", "重試", "再試行")}</button>
            </div>
          )}

          {catalogMode === "regular" && visibleRegular.length > 0 && (
            <div
              className="provider-catalog-grid"
              role="list"
              aria-label={copy("Standard providers", "常规供应商列表", "常規供應商清單", "標準プロバイダー一覧")}
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
              aria-label={copy("Free providers", "免费供应商列表", "免費供應商清單", "無料プロバイダー一覧")}
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
              <strong>{copy("No matching providers", "没有匹配的供应商", "沒有匹配的供應商", "一致するプロバイダーがありません")}</strong>
              <p>{copy(
                "No providers match the current search and filters.",
                "当前搜索词或筛选组合没有结果。", "當前搜尋詞或篩選組合沒有結果。", "現在の検索語またはフィルタの組み合わせで結果がありません。"
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
                  {copy("Clear filters", "清除筛选", "清除篩選", "フィルタをクリア")}
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
          <h1>{preset?.label ?? copy("Custom configuration", "自定义配置", "自訂配置", "カスタム設定")}</h1>
          <p>{copy(
            "Enter credentials and choose models to create a standard API instance.",
            "填写凭据并选择模型；保存后创建独立的常规 API 实例。", "填寫憑據並選擇模型；儲存後建立獨立的常規 API 例項。", "資格情報を入力し、モデルを選択してください。保存後、独立した標準APIインスタンスが作成されます。"
          )}</p>
        </div>
      </header>

      {error && <div className="banner err">{error}</div>}
      {isExisting && !error && (
        <div className="banner info">
          {copy(
            `Provider "${name.trim()}" already exists. Saving updates its base URL, API key, and models instead of creating a duplicate.`,
            `供应商“${name.trim()}”已经存在。继续保存会更新它的 Base URL、API Key 和模型，不会重复创建。`, `供應商 "${name.trim()}" 已經存在。繼續儲存會更新它的 Base URL、API Key 和模型，不會重複建立。`, `プロバイダー "${name.trim()}" はすでに存在しています。続行して保存すると、Base URL、API Key、およびモデルが更新され、重複して作成されません。`
          )}
        </div>
      )}

      <section className="provider-wizard">
        <div
          className="wizard-step"
          role="group"
          aria-label={copy("Provider credentials", "供应商凭据", "供應商憑證", "プロバイダー資格情報")}
          data-onboarding-target="provider-credential"
        >
          <div className="step-index">01</div>
          <div className="step-body form-grid provider-credential-form">
            <label className="field-label">
              {copy("Name", "名称", "名稱", "名前")}
              <input className="input" value={name} disabled={disabled || !isCustom} onChange={(event) => setName(event.target.value)} />
            </label>
            {isCustom && (
              <div className="field-label">
                <span>{copy("API dialect", "API 方言", "API 方言", "API ディアレクト")}</span>
                <Select
                  value={providerDialect}
                  disabled={disabled}
                  onValueChange={(value) => changeProviderDialect(value as typeof providerDialect)}
                >
                  <SelectTrigger className="w-full" aria-label={copy("API dialect", "API 方言", "API 方言", "API ディアレクト")}>
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
                  "Base URL 必须指向资源的 /openai/v1 根路径；模型名填写 Azure deployment name，凭据只通过 api-key 发送。", "Base URL 必須指向資源的 /openai/v1 根路徑；模型名稱請填寫 Azure 部署名稱，憑證只會透過 api-key 傳送。", "Base URL はリソースの /openai/v1 ルートを指す必要があります。モデル名には Azure のデプロイ名を入力し、認証情報は api-key でのみ送信されます。"
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
                  "填写 Base URL 后显示最终请求地址。", "填寫 Base URL 後顯示最終請求地址。", "Base URL を入力すると、最終的なリクエストエンドポイントが表示されます。"
                )}</span>
              )}
            </div>
            {needsKey ? (
              <div className="form-span credential-fields">
                {credentialSource === "store" && (
                  <label className="field-label">
                    API Key
                    <input
                      className="input mono"
                      type="password"
                      autoComplete="off"
                      placeholder={copy("Stored in local secrets.json", "保存在本机 secrets.json", "儲存在本機 secrets.json", "ローカルの secrets.json に保存")}
                      value={key}
                      disabled={disabled}
                      onChange={(event) => setKey(event.target.value)}
                    />
                  </label>
                )}
                <details className="credential-source-advanced">
                  <summary>{copy("Advanced credential source", "高级凭据来源", "高階憑證來源", "高度な資格情報ソース")}</summary>
                  <div className="field-label">
                    <span>{copy("Credential source", "凭据来源", "憑證來源", "資格情報ソース")}</span>
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
                        aria-label={copy("Credential source", "凭据来源", "憑證來源", "資格情報ソース")}
                      >
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent position="popper" align="start">
                        <SelectGroup>
                          <SelectItem value="store">
                            {copy("Local store (default)", "本地存储（默认）", "本地儲存（預設）", "ローカルストレージ（デフォルト）")}
                          </SelectItem>
                          <SelectItem value="env">
                            {copy("Environment variable", "环境变量", "環境變數", "環境変数")}
                          </SelectItem>
                          <SelectItem value="file">
                            {copy("Credential file", "凭据文件", "憑據檔案", "資格情報ファイル")}
                          </SelectItem>
                        </SelectGroup>
                      </SelectContent>
                    </Select>
                  </div>
                  {credentialSource !== "store" && (
                    <label className="field-label">
                      {credentialSource === "env"
                        ? copy("Environment variable name", "环境变量名", "環境變數名", "環境変数名")
                        : copy("Absolute credential file path", "凭据文件绝对路径", "憑據檔案絕對路徑", "絶対パスの認証ファイル")}
                      <input
                        className="input mono"
                        aria-label={credentialSource === "env"
                          ? copy("Environment variable name", "环境变量名", "環境變數名", "環境変数名")
                          : copy("Absolute credential file path", "凭据文件绝对路径", "憑據檔案絕對路徑", "絶対パスの認証ファイル")}
                        value={credentialReference}
                        disabled={disabled}
                        placeholder={credentialSource === "env" ? "DEEPSEEK_API_KEY" : "/absolute/path/provider.key"}
                        onChange={(event) => setCredentialReference(event.target.value)}
                      />
                    </label>
                  )}
                  <p className="inline-note">{copy(
                    "env/file stores only the reference. Token Station reads the value at request time.",
                    "env/file 只保存引用，Token Station 在请求时读取凭据值。", "env/file 只儲存參考，Token Station 在請求時讀取憑證值。", "env/file は参照のみを保存し、Token Station はリクエスト時に資格情報を読み込みます。"
                  )}</p>
                </details>
              </div>
            ) : (
              <div className="local-provider-note form-span">
                {copy("Local provider. No API key required.", "本地供应商，无需 API Key。", "本地供應商。無需 API Key。", "ローカルプロバイダー。API Key は必要ありません。")}
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
                "This model runs on this machine, for example through Ollama or LM Studio.",
                "该模型在本机运行，例如通过 Ollama 或 LM Studio。", "該模型在本機執行，例如透過 Ollama 或 LM Studio。", "このモデルは Ollama や LM Studio などを介してこのマシン上で実行されます。"
              )}</span>
            </label>
            <span id="provider-local-eligibility" hidden>
              {!endpointPreview
                ? copy(
                    "Resolve the base URL before marking this provider as local.",
                    "Base URL 解析完成后才能判断是否为本地模型。", "解析 Base URL 後才能標記此供應商為本地模型。", "Base URL を解析した後で、このプロバイダーをローカルプロバイダーとしてマークできます。"
                  )
                : endpointPreview.loopback
                  ? copy(
                      "A loopback endpoint was detected. This provider can be marked as local.",
                      "已检测到本机回环地址，可以标记为本地模型。", "已檢測到本機回環地址，可以標記為本地模型。", "ローカルホストアドレスが検出されました。このプロバイダーをローカルプロバイダーとしてマークできます。"
                    )
                  : copy(
                      "Cloud endpoints cannot be marked as local. Only localhost or 127.0.0.1 endpoints qualify.",
                      "云端地址不能标记为本地模型；只有 localhost 或 127.0.0.1 等回环地址可以使用此选项。", "雲端地址不能標記為本地模型；只有 localhost 或 127.0.0.1 等回環地址可以使用此選項。", "クラウドアドレスはローカルプロバイダーとしてマークできません。ローカルホスト（localhost または 127.0.0.1）などのループバックアドレスのみがこのオプションを使用できます。"
                    )}
            </span>
          </div>
        </div>

        <div
          className="wizard-step"
          role="group"
          aria-label={copy("Provider models", "供应商模型", "供應商模型", "プロバイダーのモデル")}
          data-onboarding-target="provider-models"
        >
          <div className="step-index">02</div>
          <div className="step-body provider-model-fields">
            <label className="field-label">{copy("Select models", "选择模型", "選擇模型", "モデルを選択")}</label>
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
                "批量填充匹配的公开价格", "填入匹配的公開價格", "一致する公開価格を一括で入力"
              )}</span>
            </label>
          </div>
        </div>

        <footer className="wizard-actions">
          <button className="btn" type="button" disabled={disabled} onClick={leaveRegularConfig}>
            {copy("Back to catalog", "返回目录", "返回目錄", "カタログに戻る")}
          </button>
          <button
            className="btn primary"
            type="button"
            data-onboarding-target="provider-save"
            disabled={disabled || !endpointPreview || Boolean(endpointError)}
            onClick={() => void submit()}
          >
            {saving
              ? copy("Saving…", "正在保存…", "正在儲存…", "保存中…")
              : isExisting
                ? copy("Update provider", "更新供应商", "更新供應商", "プロバイダーを更新")
                : copy("Add provider", "添加供应商", "新增供應商", "プロバイダーを追加")}
          </button>
        </footer>
      </section>
    </div>
  );
}
