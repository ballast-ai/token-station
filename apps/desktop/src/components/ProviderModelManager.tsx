import { useEffect, useId, useMemo, useState } from "react";
import {
  CatalogModelView,
  ModelDiscoveryView,
  CapabilityState,
  ModelCapabilityView,
  ProviderView,
  StateView,
  discoverProviderModels,
  editProvider,
  getState,
  getStats,
  previewProviderEndpoints,
  ProviderEndpointPreview,
  ProviderCallEngine,
  ProviderTestResult,
  setProviderModelLimits,
  setProviderModelVision,
  testProvider,
  updateProviderModels,
} from "../api";
import ModelPicker, { CatalogStatus } from "./ModelPicker";
import { useLocalizedCopy, type Language, type LocalizedCopy } from "./LanguageProvider";
import { humanizeAppError } from "../errors";
import { useErrorToast } from "./ErrorToast";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "./ui/select";

interface ProviderModelManagerProps {
  provider: ProviderView;
  serveRunning: boolean;
  disabled?: boolean;
  onSaved: (state: StateView) => void;
}

const mergeModels = (...groups: string[][]) => [...new Set(groups.flat())];

const unknownCapabilities = (model: string): ModelCapabilityView => ({
  model,
  tool: "unknown",
  vision: "unknown",
  json_schema: "unknown",
  context_window: 0,
  max_output_tokens: 0,
});

type ProviderHealth = "untested" | "healthy" | "degraded" | "unavailable";

interface ModelLimitDraft {
  context: string;
  output: string;
}

const baseLayers = new Set(["network", "http", "auth", "model", "generation"]);

function providerHealth(results: ProviderTestResult[]): ProviderHealth {
  if (results.length === 0) return "untested";
  const stages = results.flatMap((result) => result.stages);
  const base = stages.filter((stage) => baseLayers.has(stage.layer));
  if (base.length !== results.length * 5 || base.some((stage) => stage.status !== "pass")) {
    return "unavailable";
  }
  return stages.some((stage) => !baseLayers.has(stage.layer) && stage.status !== "pass")
    ? "degraded"
    : "healthy";
}

function stageTiming(
  stage: ProviderTestResult["stages"][number],
  copy: LocalizedCopy,
) {
  if (stage.status === "skipped") return copy("Not run", "未执行", "未執行", "実行されていません");
  if (stage.duration_ms == null) return copy("Not timed", "未计时", "未計時", "計時されていません");
  return `${stage.timing_kind === "cumulative" ? "≤" : ""}${stage.duration_ms}ms`;
}

function costLabel(
  costMicros: number | null,
  copy: LocalizedCopy,
): string {
  return costMicros != null && costMicros > 0
    ? copy(
      `Estimated cost ${(costMicros / 1_000_000).toFixed(4)}`,
      `估算成本 ${(costMicros / 1_000_000).toFixed(4)}`, `預估費用 ${(costMicros / 1_000_000).toFixed(4)}`, `推定費用 ${(costMicros / 1_000_000).toFixed(4)}`
    )
    : copy("Cost unknown", "成本未知", "費用未知", "費用は不明");
}

const resultStatus = (
  result: ModelDiscoveryView,
  copy: LocalizedCopy,
  language: Language,
): CatalogStatus => {
  const warning = result.warning ? humanizeAppError(result.warning, language) : result.warning;
  if (result.source === "live") {
    return {
      label: copy(`Synced ${result.models.length}`, `已同步 ${result.models.length} 个`, `已同步 ${result.models.length}`, `同期済み ${result.models.length}`),
      tone: "live",
      warning,
    };
  }
  if (result.source === "cache") {
    return {
      label: copy(`Using cache · ${result.models.length}`, `使用缓存 · ${result.models.length} 个`, `使用快取 · ${result.models.length}`, `キャッシュを使用 · ${result.models.length}`),
      tone: "cache",
      warning,
    };
  }
  if (result.source === "preset") {
    return {
      label: copy("Using built-in presets", "使用内置预设", "使用內建預設", "内蔵のプリセットを使用"),
      tone: "cache",
      warning,
    };
  }
  return { label: copy("Fetch failed", "获取失败", "取得失敗", "取得失敗"), tone: "error", warning };
};

export default function ProviderModelManager({
  provider,
  serveRunning,
  disabled = false,
  onSaved,
}: ProviderModelManagerProps) {
  const { copy, language } = useLocalizedCopy();
  const { showError, showSuccess } = useErrorToast();
  const endpointErrorId = useId();
  const providerCallName = useId();
  const [models, setModels] = useState(provider.models);
  const [selected, setSelected] = useState(provider.models);
  const [status, setStatus] = useState<CatalogStatus>({
    label: copy(`Configured ${provider.models.length}`, `已配置 ${provider.models.length} 个`, `已配置 ${provider.models.length}`, `設定済み ${provider.models.length}`),
    tone: "idle",
  });
  const [refreshing, setRefreshing] = useState(false);
  const [saving, setSaving] = useState(false);
  const [catalog, setCatalog] = useState<CatalogModelView[]>(provider.catalog ?? []);
  const [diff, setDiff] = useState<{ added: string[]; removed: string[] } | null>(null);
  const [endpointPreview, setEndpointPreview] = useState<ProviderEndpointPreview | null>(null);
  const [endpointError, setEndpointError] = useState("");
  const [testResults, setTestResults] = useState<ProviderTestResult[]>([]);
  const [testedAtMs, setTestedAtMs] = useState<number | null>(null);
  const [testing, setTesting] = useState(false);
  const [capabilitySaving, setCapabilitySaving] = useState<string | null>(null);
  const [limitSaving, setLimitSaving] = useState<string | null>(null);
  const [limitDrafts, setLimitDrafts] = useState<Record<string, ModelLimitDraft>>({});
  const [limitErrors, setLimitErrors] = useState<Record<string, string>>({});
  const [usage, setUsage] = useState<string>(copy(
    "Reading metadata-only usage…",
    "正在读取无正文用量…", "僅讀取後設資料用量…", "メタデータのみの使用量を読み込み中…"
  ));
  const [editBaseUrl, setEditBaseUrl] = useState(provider.base_url);
  const [editKey, setEditKey] = useState("");
  const [credentialSource, setCredentialSource] = useState<"store" | "env" | "file" | "none">(
    provider.credential_source ?? (provider.has_auth ? "store" : "none"),
  );
  const [credentialReference, setCredentialReference] = useState(
    provider.credential_reference ?? "",
  );
  const [providerCall, setProviderCall] = useState<ProviderCallEngine>(
    provider.provider_call ?? "legacy",
  );
  const [runtimeOpen, setRuntimeOpen] = useState(provider.provider_call !== undefined
    && provider.provider_call !== "legacy");
  const [editing, setEditing] = useState(false);
  const selectedSet = useMemo(() => new Set(selected), [selected]);
  const capabilities = useMemo(() => {
    const byModel = new Map((provider.model_capabilities ?? []).map((item) => [item.model, item]));
    return provider.models.map((model) => byModel.get(model) ?? unknownCapabilities(model));
  }, [provider.model_capabilities, provider.models]);
  // Merge catalog status and capabilities into one row per model to avoid duplicate tables.
  const modelRows = useMemo(() => {
    const catalogByModel = new Map(catalog.map((entry) => [entry.model, entry]));
    const capByModel = new Map(capabilities.map((cap) => [cap.model, cap]));
    return mergeModels(catalog.map((c) => c.model), capabilities.map((c) => c.model)).map((model) => ({
      model,
      configured: provider.models.includes(model),
      catalog: catalogByModel.get(model) ?? null,
      cap: capByModel.get(model) ?? unknownCapabilities(model),
    }));
  }, [catalog, capabilities, provider.models]);
  const operationDisabled = disabled || refreshing || saving || testing || editing
    || capabilitySaving !== null || limitSaving !== null;
  const capabilityLabel: Record<CapabilityState, string> = {
    verified: copy("Verified", "已验证", "已驗證", "確認済み"),
    declared: copy("Declared", "已声明", "已宣告", "宣言済み"),
    unsupported: copy("Unsupported", "不支持", "不支援", "非対応"),
    unknown: copy("Unknown", "未知", "未知", "不明"),
  };
  const limitSourceLabel = {
    provider: copy("Provider API", "供应商接口", "供應商 API", "プロバイダー API"),
    builtin_preset: copy("Built-in preset (not live)", "内置预设（非实时值）", "內建預設（非即時值）", "内蔵プリセット（非リアルタイム値）"),
    operator: copy("Manual configuration", "手动配置", "手動配置", "手動設定"),
    heuristic: copy("Heuristic default", "启发式默认", "啟發式預設", "ヒューリスティックデフォルト"),
  } as const;
  const catalogStateLabel = {
    active: copy("Active", "在售", "啟用", "アクティブ"),
    stale: copy("Cached, pending confirmation", "缓存待确认", "已快取，待確認", "キャッシュ済み、確認待ち"),
    removed: copy("Removed", "已下架", "已移除", "削除済み"),
  } as const;
  const catalogSourceLabel = {
    live: copy("Live catalog", "实时目录", "即時目錄", "リアルタイムカタログ"),
    cache: copy("Local cache", "本地缓存", "本地快取", "ローカルキャッシュ"),
    configured: copy("Manual", "手工配置", "手動", "手動"),
  } as const;
  const probeLayerLabel = {
    network: copy("DNS / Network", "DNS / 网络", "DNS / 網路", "DNS / ネットワーク"),
    http: "HTTP",
    auth: copy("Authentication", "鉴权", "驗證", "認証"),
    model: copy("Model", "模型", "模型", "モデル"),
    generation: copy("Generation", "生成", "生成", "生成"),
    stream: copy("Streaming", "流式", "流式", "ストリーム"),
    tool: "Tool",
    json: "JSON Schema",
  } as const;
  const healthLabel: Record<ProviderHealth, string> = {
    untested: copy("Untested", "未测试", "未測試", "未テスト"),
    healthy: copy("Healthy", "健康", "健康", "健康"),
    degraded: copy("Degraded capabilities", "能力退化", "能力退化", "能力低下"),
    unavailable: copy("Unavailable", "不可用", "不可用", "利用不可"),
  };
  const reportedSouthReason = provider.south_v1_unavailable_reason
    ?? (provider.south_v1_available === true ? null : "provider_package");
  const fixedSouthReason = reportedSouthReason === "auth" ? null : reportedSouthReason;
  const southUnavailableReason = fixedSouthReason
    ?? ((credentialSource === "store" || credentialSource === "env") ? null : "auth");
  const reportedHeaderAuthReason = provider.south_header_auth_v1_unavailable_reason
    ?? (provider.south_header_auth_v1_available === true ? null : "provider_package");
  const fixedHeaderAuthReason = reportedHeaderAuthReason === "auth" ? null : reportedHeaderAuthReason;
  const headerAuthUnavailableReason = fixedHeaderAuthReason
    ?? ((credentialSource === "store" || credentialSource === "env") ? null : "auth");
  const southUnavailableCopy = {
    provider_package: copy(
      "South requires the verified official OpenAI-compatible provider package.",
      "South 需要已验证的官方 OpenAI-compatible Provider 包。", "South 需要已驗證的官方 OpenAI 相容供應商套件。", "South には検証済みの公式 OpenAI 互換プロバイダーパッケージが必要です。"
    ),
    api_dialect: copy(
      "South is available only for the translated API dialect.",
      "South 仅支持 translated API dialect。", "South 僅支援轉換後的 API 方言。", "South は変換済みの API 方言にのみ対応しています。"
    ),
    egress: copy(
      "South currently requires direct egress.",
      "South 当前仅支持 direct egress。", "South 目前僅支援直接連線。", "South は現在、直接接続でのみ利用できます。"
    ),
    auth: copy(
      "South requires Bearer credentials from the local store or an environment variable.",
      "South 需要来自本地存储或环境变量的 Bearer 凭据。", "South 需要本機儲存區或環境變數中的 Bearer 憑證。", "South にはローカルストアまたは環境変数の Bearer 認証情報が必要です。"
    ),
  } as const;
  const headerAuthUnavailableCopy = {
    provider_package: copy(
      "South Header Auth requires the verified official package for this Provider dialect.",
      "South Header Auth 需要该 Provider dialect 对应的已验证官方包。", "South Header Auth 需要與此供應商方言相符的已驗證官方套件。", "South Header Auth には、このプロバイダー方言に対応する検証済み公式パッケージが必要です。"
    ),
    api_dialect: copy(
      "South Header Auth is available only for the translated API dialect.",
      "South Header Auth 仅支持 translated API dialect。", "South Header Auth 僅支援轉換後的 API 方言。", "South Header Auth は変換済みの API 方言にのみ対応しています。"
    ),
    egress: copy(
      "South Header Auth currently requires direct egress.",
      "South Header Auth 当前仅支持 direct egress。", "South Header Auth 目前僅支援直接連線。", "South Header Auth は現在、直接接続でのみ利用できます。"
    ),
    auth: copy(
      "South Header Auth requires credentials from the local store or an environment variable.",
      "South Header Auth 需要来自本地存储或环境变量的凭据。", "South Header Auth 需要本機儲存區或環境變數中的憑證。", "South Header Auth にはローカルストアまたは環境変数の認証情報が必要です。"
    ),
  } as const;
  const usesHeaderAuthEngine = providerCall === "south_v1_buffered_streaming_header_auth";
  const activeUnavailableReason = usesHeaderAuthEngine
    ? headerAuthUnavailableReason
    : southUnavailableReason;
  const activeUnavailableCopy = usesHeaderAuthEngine
    ? headerAuthUnavailableCopy
    : southUnavailableCopy;

  useEffect(() => {
    let active = true;
    setEndpointPreview(null);
    setEndpointError("");
    void previewProviderEndpoints(editBaseUrl)
      .then((preview) => {
        if (!active) return;
        setEndpointPreview(preview);
      })
      .catch((caught) => {
        if (!active) return;
        setEndpointError(humanizeAppError(caught));
      });
    return () => {
      active = false;
    };
  }, [editBaseUrl]);

  useEffect(() => {
    void getStats("all", "upstream")
      .then((view) => {
        const aggregate = view.groups.find(([name]) => name === provider.name)?.[1];
        setUsage(aggregate
          ? copy(
            `${aggregate.requests} requests · ${aggregate.errors} errors · P95 ${aggregate.p95_latency_ms}ms · ${aggregate.input_tokens + aggregate.output_tokens} tokens · ${costLabel(aggregate.cost_micros, copy)}`,
            `${aggregate.requests} 次请求 · ${aggregate.errors} 次错误 · P95 ${aggregate.p95_latency_ms}ms · ${aggregate.input_tokens + aggregate.output_tokens} tokens · ${costLabel(aggregate.cost_micros, copy)}`, `${aggregate.requests} 個請求 · ${aggregate.errors} 個錯誤 · P95 ${aggregate.p95_latency_ms}ms · ${aggregate.input_tokens + aggregate.output_tokens} 個 Token · ${costLabel(aggregate.cost_micros, copy)}`, `${aggregate.requests} 回のリクエスト · ${aggregate.errors} 回のエラー · P95 ${aggregate.p95_latency_ms}ms · ${aggregate.input_tokens + aggregate.output_tokens} 個のトークン · ${costLabel(aggregate.cost_micros, copy)}`
          )
          : copy("No request records", "暂无请求记录", "無請求紀錄", "リクエスト記録がありません"));
      })
      .catch(() => setUsage(copy("Usage is temporarily unavailable", "用量暂不可读", "用量暫時不可讀", "使用状況は一時的に読み取れません")));
  }, [copy, provider.name]);

  useEffect(() => {
    setLimitDrafts(Object.fromEntries(capabilities.map((capability) => [
      capability.model,
      {
        context: capability.context_window ? String(capability.context_window) : "",
        output: capability.max_output_tokens ? String(capability.max_output_tokens) : "",
      },
    ])));
    setLimitErrors({});
  }, [capabilities]);

  const updateLimitDraft = (model: string, field: keyof ModelLimitDraft, value: string) => {
    setLimitDrafts((current) => ({
      ...current,
      [model]: {
        context: current[model]?.context ?? "",
        output: current[model]?.output ?? "",
        [field]: value,
      },
    }));
    setLimitErrors((current) => ({ ...current, [model]: "" }));
  };

  const saveLimits = async (model: string) => {
    const draft = limitDrafts[model] ?? { context: "", output: "" };
    const context = Number(draft.context);
    const output = Number(draft.output);
    let error = "";
    if (!Number.isSafeInteger(context) || !Number.isSafeInteger(output)
      || context <= 0 || output <= 0 || context > 0xffff_ffff || output > 0xffff_ffff) {
      error = copy(
        "Context and maximum output tokens must be positive integers.",
        "上下文上限和最大输出 Token 必须是大于 0 的整数。", "上下文上限和最大輸出 Token 必須是大於 0 的整數。", "コンテキスト上限と最大出力トークンは0より大きい整数でなければなりません。"
      );
    } else if (output > context) {
      error = copy(
        "Maximum output tokens cannot exceed the context window.",
        "最大输出 Token 不能大于上下文上限。", "最大輸出 Token 不能大於上下文上限。", "最大出力トークンはコンテキスト上限を超えてはなりません。"
      );
    }
    if (error) {
      setLimitErrors((current) => ({ ...current, [model]: error }));
      return;
    }
    setLimitSaving(model);
    try {
      const next = await setProviderModelLimits(provider.name, model, context, output);
      onSaved(next);
      showSuccess(
        serveRunning
          ? copy("Model limits saved; restart the proxy to apply.", "已保存模型限制；重启代理后生效", "模型限制已儲存；重啟代理後生效。", "モデルの制限が保存されました。プロキシを再起動してから有効になります。")
          : copy("Model limits saved.", "已保存模型限制", "模型限制已儲存。", "モデルの制限が保存されました。"),
        `provider-model-limits:${provider.name}:${model}`,
      );
    } catch (caught) {
      showError(
        humanizeAppError(caught, language),
        `provider-model-limits:${provider.name}:${model}`,
      );
    } finally {
      setLimitSaving(null);
    }
  };

  const saveProviderDetails = async () => {
    if (operationDisabled || !endpointPreview) return;
    setEditing(true);
    try {
      onSaved(await editProvider(
        provider.name,
        editBaseUrl,
        credentialSource === "store" ? editKey.trim() || null : null,
        credentialSource,
        credentialSource === "env" || credentialSource === "file"
          ? credentialReference.trim()
          : null,
        providerCall,
      ));
      setEditKey("");
      showSuccess(
        copy(`Provider ${provider.name} details saved`, `${provider.name} 的基本信息已保存`, `${provider.name} 的基本資訊已儲存`, `${provider.name} の基本情報が保存されました`),
        `provider-edit:${provider.name}`,
      );
    } catch (caught) {
      showError(humanizeAppError(caught), `provider-edit:${provider.name}`);
    } finally {
      setEditing(false);
    }
  };

  const runProviderTest = async () => {
    if (operationDisabled) return;
    setTesting(true);
    try {
      const results = await testProvider(provider.name);
      setTestResults(results);
      setTestedAtMs(Date.now());
    } catch (caught) {
      showError(humanizeAppError(caught), `provider-test:${provider.name}`);
    } finally {
      setTesting(false);
    }
  };

  const toggleVision = async (model: string, state: CapabilityState) => {
    if (operationDisabled || state === "verified") return;
    setCapabilitySaving(model);
    try {
      onSaved(await setProviderModelVision(provider.name, model, state !== "declared"));
    } catch (caught) {
      showError(humanizeAppError(caught), `provider-capability:${provider.name}:${model}`);
    } finally {
      setCapabilitySaving(null);
    }
  };

  const refresh = async () => {
    if (operationDisabled) return;
    if (provider.provider === "azure-openai-v1") {
      setStatus({
        label: copy("Manual deployment", "手工填写 deployment", "手動部署", "手動デプロイ"),
        tone: "idle",
        warning: humanizeAppError("model_catalog_azure_deployment_manual", language),
      });
      return;
    }
    setRefreshing(true);
    setStatus({ label: copy("Fetching…", "正在获取…", "正在取得…", "取得中…"), tone: "loading" });
    try {
      const result = await discoverProviderModels(provider.name, provider.base_url, null);
      setModels((current) => mergeModels(current, result.models));
      setCatalog(result.catalog ?? []);
      setDiff({ added: result.added ?? [], removed: result.removed ?? [] });
      setStatus(resultStatus(result, copy, language));
      if (result.capabilities_updated) {
        onSaved(await getState());
      }
      showSuccess(
        copy(`Models for ${provider.name} refreshed`, `${provider.name} 的模型目录已刷新`, `${provider.name} 的模型已更新`, `${provider.name} のモデルが更新されました`),
        `provider-catalog:${provider.name}`,
      );
    } catch (caught) {
      const message = humanizeAppError(caught);
      setStatus({ label: copy("Fetch failed", "获取失败", "取得失敗", "取得失敗"), tone: "error", warning: null });
      showError(message, `provider-catalog:${provider.name}`);
    } finally {
      setRefreshing(false);
    }
  };

  const save = async () => {
    if (operationDisabled) return;
    setSaving(true);
    try {
      const next = await updateProviderModels(provider.name, selected);
      onSaved(next);
      setStatus({
        label: copy(`Configured ${selected.length}`, `已配置 ${selected.length} 个`, `已配置 ${selected.length}`, `設定済み ${selected.length}`),
        tone: "idle",
      });
      showSuccess(
        copy(`Saved ${selected.length} models`, `已保存 ${selected.length} 个模型`, `已儲存 ${selected.length} 個模型`, `${selected.length} 個モデルが保存されました`),
        `provider-models-save:${provider.name}`,
      );
    } catch (caught) {
      showError(humanizeAppError(caught), `provider-models-save:${provider.name}`);
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="provider-model-manager">
      <div className="provider-detail-summary">
        <div>
          <strong>{copy("Provider details", "供应商详情", "供應商詳情", "プロバイダーの詳細")}</strong>
          <span>{usage}</span>
        </div>
        <div className="provider-health-actions">
          <div
            className={`provider-health-badge ${providerHealth(testResults)}`}
            aria-label={copy(
              `Provider health: ${healthLabel[providerHealth(testResults)]}`,
              `供应商健康状态：${healthLabel[providerHealth(testResults)]}`, `供應商健康狀態：${healthLabel[providerHealth(testResults)]}`, `プロバイダーの健康状態：${healthLabel[providerHealth(testResults)]}`
            )}
          >
            <i aria-hidden="true" />
            <span>{healthLabel[providerHealth(testResults)]}</span>
            {testedAtMs != null && (
              <small>{copy("Last tested", "最近测试", "最近測試", "最近のテスト")} {new Date(testedAtMs).toLocaleString(language === "zh-CN" ? "zh-CN" : "en-US")}</small>
            )}
          </div>
          <button className="btn tiny" type="button" disabled={operationDisabled} onClick={() => void runProviderTest()}>
            {testing ? copy("Testing…", "测试中…", "測試中…", "テスト中…") : copy("Run layered test", "运行分层测试", "執行分層測試", "レイヤードテストを実行")}
          </button>
          <small className="provider-test-charge-warning">{copy(
            "This sends real Provider requests and may incur charges.",
            "该测试会向真实 Provider 发出请求，可能产生费用。", "此測試會向真實供應商發出請求，可能產生費用。", "このテストは実際のプロバイダーにリクエストを送信し、費用が発生する可能性があります。"
          )}</small>
        </div>
      </div>
      <div className="provider-edit-fields">
        <input
          className="input mono"
          aria-label={copy("Edit base URL", "编辑 Base URL", "編輯 Base URL", "Base URLを編集")}
          aria-invalid={Boolean(endpointError)}
          aria-describedby={endpointError ? endpointErrorId : undefined}
          value={editBaseUrl}
          disabled={operationDisabled}
          onChange={(event) => setEditBaseUrl(event.target.value)}
        />
        <Select
          value={credentialSource}
          disabled={operationDisabled}
          onValueChange={(value) => {
            const nextSource = value as typeof credentialSource;
            setCredentialSource(nextSource);
            if (nextSource !== "store" && nextSource !== "env") {
              setProviderCall("legacy");
            }
            setEditKey("");
            setCredentialReference("");
          }}
        >
          <SelectTrigger
            className="provider-credential-select"
            aria-label={copy("Edit credential source", "编辑凭据来源", "編輯憑據來源", "資格情報のソースを編集")}
          >
            <SelectValue />
          </SelectTrigger>
          <SelectContent position="popper" align="start">
            <SelectItem value="store">{copy("Local store (default)", "本地存储（默认）", "本地儲存（預設）", "ローカルストレージ（デフォルト）")}</SelectItem>
            <SelectItem value="env">{copy("Environment variable", "环境变量", "環境變數", "環境変数")}</SelectItem>
            <SelectItem value="file">{copy("Credential file", "凭据文件", "憑據檔案", "資格情報ファイル")}</SelectItem>
            <SelectItem value="none">{copy("No authentication", "无鉴权", "無認證", "認証なし")}</SelectItem>
          </SelectContent>
        </Select>
        {credentialSource === "store" && (
          <input className="input mono" aria-label={copy("Update API key", "更新 API Key", "更新 API Key", "API Keyを更新")} type="password" value={editKey} disabled={operationDisabled} placeholder={copy("Leave blank to keep the current key", "留空则保留现有 Key", "留空則保留現有 Key", "空白にすると現在のKeyを保持")} onChange={(event) => setEditKey(event.target.value)} />
        )}
        {(credentialSource === "env" || credentialSource === "file") && (
          <input
            className="input mono"
            aria-label={credentialSource === "env"
              ? copy("Environment variable name", "环境变量名", "環境變數名", "環境変数名")
              : copy("Absolute credential file path", "凭据文件绝对路径", "憑據檔案絕對路徑", "絶対パスの認証ファイル")}
            value={credentialReference}
            disabled={operationDisabled}
            placeholder={credentialSource === "env" ? "DEEPSEEK_API_KEY" : "/absolute/path/provider.key"}
            onChange={(event) => setCredentialReference(event.target.value)}
          />
        )}
        <button className="btn tiny" type="button" disabled={operationDisabled || !endpointPreview} onClick={() => void saveProviderDetails()}>
          {editing ? copy("Saving…", "保存中…", "儲存中…", "保存中…") : copy("Save details", "保存基本信息", "儲存詳細資訊", "詳細情報を保存")}
        </button>
      </div>
      <details
        className={`provider-runtime-advanced ${providerCall === "legacy" ? "stable" : "experimental"}`}
        open={runtimeOpen}
        onToggle={(event) => setRuntimeOpen(event.currentTarget.open)}
      >
        <summary>
          <span>{copy("Advanced runtime", "高级运行时", "進階執行時", "アドバンスド実行時")}</span>
          <small>
            {providerCall === "legacy"
              ? copy("Legacy active", "Legacy 已启用", "遺產已啟用", "レガシーが有効")
              : activeUnavailableReason
                ? copy(
                  "Experimental configured but unavailable",
                  "实验运行时已配置但不可用", "實驗執行時已配置但不可用", "実験的実行時が設定済みですが利用不可"
                )
                : copy("Experimental active", "实验运行时已启用", "實驗執行時已啟用", "実験的実行時が有効")}
          </small>
        </summary>
        <div className="provider-runtime-panel">
          <p>{copy(
            "Choose how this provider sends requests. Legacy remains the stable default.",
            "选择此 Provider 发送请求的方式。Legacy 始终是稳定默认项。", "選擇此供應商傳送請求的方式。Legacy 始終是穩定預設選項。", "このプロバイダーがリクエストを送信する方法を選択してください。レガシーは常に安定したデフォルトです。"
          )}</p>
          <fieldset className="provider-runtime-options">
            <legend>{copy("Provider runtime", "Provider 运行时", "供應商執行時", "プロバイダー実行時")}</legend>
            <label className={providerCall === "legacy" ? "selected" : ""}>
              <input
                type="radio"
                name={providerCallName}
                value="legacy"
                checked={providerCall === "legacy"}
                disabled={operationDisabled}
                onChange={() => setProviderCall("legacy")}
              />
              <span>
                <strong>Legacy</strong>
                <small>{copy("Stable default · broad compatibility", "稳定默认 · 兼容性最广", "穩定預設 · 相容性最廣", "安定したデフォルト · 最大の互換性")}</small>
              </span>
              <i>{copy("Stable", "稳定", "穩定", "安定")}</i>
            </label>
            <label className={providerCall === "south_v1_buffered" ? "selected" : ""}>
              <input
                type="radio"
                name={providerCallName}
                value="south_v1_buffered"
                checked={providerCall === "south_v1_buffered"}
                disabled={operationDisabled || southUnavailableReason !== null}
                onChange={() => setProviderCall("south_v1_buffered")}
              />
              <span>
                <strong>{copy("South buffered only", "South 仅非流式", "South 僅非流式", "South は非ストリームのみ")}</strong>
                <small>{copy(
                  "South handles eligible buffered requests. Streams remain on Legacy.",
                  "符合条件的非流式请求使用 South。流式请求仍使用 Legacy。", "South 處理符合條件的非流式請求。流式請求仍使用 Legacy。", "South は条件に合致した非ストリームリクエストを処理します。ストリームリクエストはレガシーが使用されます。"
                )}</small>
              </span>
              <i>{copy("Experimental", "实验", "實驗", "実験")}</i>
            </label>
            <label className={providerCall === "south_v1_buffered_streaming" ? "selected" : ""}>
              <input
                type="radio"
                name={providerCallName}
                value="south_v1_buffered_streaming"
                checked={providerCall === "south_v1_buffered_streaming"}
                disabled={operationDisabled || southUnavailableReason !== null}
                onChange={() => setProviderCall("south_v1_buffered_streaming")}
              />
              <span>
                <strong>{copy("South buffered + streaming", "South 非流式 + 流式", "South 非流式 + 流式", "South 非ストリーム + ストリーム")}</strong>
                <small>{copy(
                  "South for eligible buffered and streaming requests.",
                  "符合条件的非流式和流式请求均使用 South。", "符合條件的非流式和流式請求均使用 South。", "条件に合致した非ストリームおよびストリームリクエストはすべて South が使用されます。"
                )}</small>
              </span>
              <i>{copy("Experimental", "实验", "實驗", "実験")}</i>
            </label>
            <label className={usesHeaderAuthEngine ? "selected" : ""}>
              <input
                type="radio"
                name={providerCallName}
                value="south_v1_buffered_streaming_header_auth"
                checked={usesHeaderAuthEngine}
                disabled={operationDisabled || headerAuthUnavailableReason !== null}
                onChange={() => setProviderCall("south_v1_buffered_streaming_header_auth")}
              />
              <span>
                <strong>{copy(
                  "South buffered + streaming + Header Auth",
                  "South 非流式 + 流式 + Header Auth", "南向緩衝 + 流式 + Header 身分驗證", "南向バッファ + ストリーミング + ヘッダー認証"
                )}</strong>
                <small>{copy(
                  "Adds fixed api-key Header Auth for eligible Azure OpenAI v1 calls.",
                  "为符合条件的 Azure OpenAI v1 调用启用固定 api-key Header Auth。", "為符合條件的 Azure OpenAI v1 呼叫啟用固定 api-key Header 身分驗證。", "条件を満たす Azure OpenAI v1 呼び出しで、固定 api-key ヘッダー認証を有効にします。"
                )}</small>
              </span>
              <i>{copy("Experimental", "实验", "實驗", "実験")}</i>
            </label>
          </fieldset>
          <p className={`provider-runtime-note ${providerCall === "legacy" ? "" : "warning"}`}>
            {activeUnavailableReason
              ? activeUnavailableCopy[activeUnavailableReason]
              : providerCall === "legacy"
                ? copy(
                  "South is eligible for this provider. Enable it only for a controlled canary.",
                  "此 Provider 符合 South 条件。仅在受控 canary 中启用。", "此供應商符合 South 條件。請只在受控的金絲雀測試中啟用。", "このプロバイダーは South の条件を満たしています。管理されたカナリアテストでのみ有効にしてください。"
                )
                : usesHeaderAuthEngine
                  ? copy(
                    "Eligible calls use South. Azure OpenAI v1 uses the fixed api-key header. South attempts never replay through Legacy.",
                    "符合条件的调用使用 South。Azure OpenAI v1 使用固定 api-key header。South 尝试不会通过 Legacy 重放。", "符合資格的呼叫使用 South。Azure OpenAI v1 使用固定 api-key header。South 嘗試不會通過 Legacy 重放。", "資格のある呼び出しは South を使用します。Azure OpenAI v1 は固定 api-key ヘッダーを使用します。South の試行は Legacy を経由して再送信されません。"
                  )
                : copy(
                  "South never replays an attempt through Legacy once South execution begins.",
                  "South 一旦开始执行，就不会再通过 Legacy 重放该次尝试。", "South 一旦開始執行，就不會再通過 Legacy 重放該次嘗試。", "South が実行を開始した後は、Legacy を経由して再送信されません。"
                )}
          </p>
        </div>
      </details>
      {endpointError && (
        <p id={endpointErrorId} className="error-text" role="alert">
          {endpointError}
        </p>
      )}
      {endpointPreview && (
        <div className="provider-endpoint-list" aria-label={copy("Final provider URLs", "供应商最终地址", "最終提供者 URL", "最終プロバイダー URL")}>
          <code>{endpointPreview.chat}</code>
          <code>{endpointPreview.responses}</code>
          <code>{endpointPreview.messages}</code>
        </div>
      )}
      {testResults.length > 0 && (
        <div className="provider-test-results" aria-label={copy("Provider layered test results", "供应商分层测试结果", "提供者分層測試結果", "プロバイダー分層テスト結果")}>
          <p>{copy(
            "The five base layers show cumulative timing (≤) for one live generation probe; capability layers show their own request timing.",
            "基础五层显示同一次真实生成探测的累计耗时（≤）；能力层显示各自真实请求耗时。", "基礎五層顯示同一次真實生成探測的累計耗時（≤）；能力層顯示各自真實請求耗時。", "基礎五層は、1回のリアルタイム生成プローブの累積時間（≤）を表示し、能力層はそれぞれのリアルタイムリクエスト時間を見ます。"
          )}</p>
          {testResults.map((result) => (
            <div key={result.model}>
              <strong>{result.model}</strong>
              {result.stages.map((stage) => (
                <span className={stage.status} key={stage.layer} title={stage.detail}>
                  {probeLayerLabel[stage.layer]} · {stage.status} · {stageTiming(stage, copy)}
                </span>
              ))}
            </div>
          ))}
        </div>
      )}
      {diff && (diff.added.length > 0 || diff.removed.length > 0) && (
        <div className="catalog-diff" aria-label={copy("Model catalog changes", "模型目录变化", "模型目錄變化", "モデルカタログの変更")}>
          {diff.added.length > 0 && (
            <span className="added">{copy(
              `Added: ${diff.added.join(", ")}`,
              `新增：${diff.added.join("、")}`, `新增：${diff.added.join(", ")}`, `追加：${diff.added.join(", ")}`
            )}</span>
          )}
          {diff.removed.length > 0 && (
            <span className="removed">{copy(
              `Removed: ${diff.removed.join(", ")} (references retained)`,
              `下架：${diff.removed.join("、")}（仍保留引用）`, `下架：${diff.removed.join(", ")}（仍保留引用）`, `削除：${diff.removed.join(", ")}（参照は保持されます）`
            )}</span>
          )}
        </div>
      )}
      {modelRows.length > 0 && (
        <div className="model-ledger" aria-label={copy("Model catalog and capabilities", "模型目录与能力", "模型目錄與能力", "モデルカタログと能力")}>
          {modelRows.map((row) => (
            <div className={`model-ledger-row ${row.catalog?.catalog_state ?? ""}`} key={row.model}>
              <code>{row.model}</code>
              <div className="model-ledger-tags">
                {row.catalog && (
                  <span className="model-ledger-state">
                    {catalogStateLabel[row.catalog.catalog_state]} · {catalogSourceLabel[row.catalog.source]}
                  </span>
                )}
                {([
                  [copy("Tools", "工具", "工具", "ツール"), row.cap.tool],
                  [copy("Vision", "视觉", "視覺", "ビジョン"), row.cap.vision],
                  ["JSON", row.cap.json_schema],
                ] as const).map(([label, state]) => (
                  label === copy("Vision", "视觉", "視覺", "ビジョン") ? (
                    <button
                      aria-label={state === "verified"
                        ? copy(`${row.model} vision capability verified`, `${row.model} 视觉能力已验证`, `${row.model} 視覺能力已驗證`, `${row.model} のビジョン能力が確認されました`)
                        : state === "declared"
                          ? copy(`Mark ${row.model} as not supporting vision`, `将 ${row.model} 标记为不支持视觉`, `將 ${row.model} 標記為不支援視覺`, `${row.model} をビジョンをサポートしないとマークします`)
                          : copy(`Declare vision support for ${row.model}`, `为 ${row.model} 声明视觉支持`, `為 ${row.model} 宣告視覺支援`, `${row.model} に視覚サポートを宣言`)}
                      aria-pressed={state === "verified" || state === "declared"}
                      className={`capability-tag capability-tag-button ${state}`}
                      disabled={operationDisabled || state === "verified"}
                      key={label}
                      onClick={() => void toggleVision(row.model, state)}
                      type="button"
                    >
                      {label} · {capabilitySaving === row.model ? copy("Saving…", "保存中…", "儲存中…", "保存中…") : capabilityLabel[state]}
                    </button>
                  ) : (
                    <span className={`capability-tag ${state}`} key={label}>
                      {label} · {capabilityLabel[state]}
                    </span>
                  )
                ))}
              </div>
              {row.configured && row.cap.context_window_source
                && row.cap.context_window_source === row.cap.max_output_tokens_source && (
                <p className="model-limit-source">
                  {copy("Limit source", "限制来源", "限制來源", "ソースを制限")} · {limitSourceLabel[row.cap.context_window_source]}
                </p>
              )}
              {row.configured && row.cap.context_window_source
                && row.cap.max_output_tokens_source
                && row.cap.context_window_source !== row.cap.max_output_tokens_source && (
                <p className="model-limit-source">
                  {copy("Context", "上下文", "上下文", "コンテキスト")} · {limitSourceLabel[row.cap.context_window_source]}
                  {" · "}
                  {copy("Output", "输出", "輸出", "出力")} · {limitSourceLabel[row.cap.max_output_tokens_source]}
                </p>
              )}
              {row.configured && <div className="model-limit-editor">
                <label>
                  <span>{copy("Context", "上下文上限", "上下文上限", "コンテキスト上限")}</span>
                  <input
                    aria-label={copy(`${row.model} context window`, `${row.model} 上下文上限`, `${row.model} 上下文上限`, `${row.model} コンテキスト上限`)}
                    className="input mono"
                    disabled={operationDisabled}
                    inputMode="numeric"
                    min={1}
                    max={0xffff_ffff}
                    onChange={(event) => updateLimitDraft(row.model, "context", event.target.value)}
                    step={1}
                    type="number"
                    value={limitDrafts[row.model]?.context ?? ""}
                  />
                </label>
                <label>
                  <span>{copy("Maximum output", "最大输出 Token", "最大輸出", "最大出力")}</span>
                  <input
                    aria-label={copy(`${row.model} maximum output tokens`, `${row.model} 最大输出 Token`, `${row.model} 最大輸出 Token`, `${row.model} 最大出力トークン`)}
                    className="input mono"
                    disabled={operationDisabled}
                    inputMode="numeric"
                    min={1}
                    max={0xffff_ffff}
                    onChange={(event) => updateLimitDraft(row.model, "output", event.target.value)}
                    step={1}
                    type="number"
                    value={limitDrafts[row.model]?.output ?? ""}
                  />
                </label>
                <button
                  aria-label={copy(`Save ${row.model} model limits`, `保存 ${row.model} 模型限制`, `儲存 ${row.model} 模型限制`, `${row.model} のモデル制限を保存`)}
                  className="btn tiny"
                  disabled={operationDisabled}
                  onClick={() => void saveLimits(row.model)}
                  type="button"
                >
                  {limitSaving === row.model ? copy("Saving…", "保存中…", "儲存中…", "保存中…") : copy("Save limits", "保存限制", "儲存限制", "制限を保存")}
                </button>
              </div>}
              {row.configured && !row.cap.max_output_tokens && (
                <p className="model-limit-warning">
                  {copy(
                    "This model's metadata is missing a maximum output limit.",
                    "该模型元数据缺少最大输出上限", "此模型的後設資料缺少最大輸出上限。", "このモデルのメタデータに最大出力上限がありません。"
                  )}
                </p>
              )}
              {row.configured && limitErrors[row.model] && (
                <p className="field-error" role="alert">{limitErrors[row.model]}</p>
              )}
            </div>
          ))}
        </div>
      )}
      <ModelPicker
        models={models}
        selected={selected}
        status={status}
        refreshing={refreshing}
        disabled={disabled || saving}
        onRefresh={refresh}
        onToggle={(model) =>
          setSelected((current) =>
            current.includes(model) ? current.filter((candidate) => candidate !== model) : [...current, model],
          )
        }
        onAdd={(model) => {
          setModels((current) => mergeModels(current, [model]));
          if (!selectedSet.has(model)) setSelected((current) => [...current, model]);
        }}
      />
      <div className="manager-actions">
        <span className="manager-hint">
          {serveRunning
            ? copy("Proxy running · Restart after saving to apply", "代理运行中 · 保存后重启代理生效", "代理正在執行 · 儲存後重啟代理生效", "プロキシが実行中 · 保存後プロキシを再起動して有効にする")
            : copy("Save to write the current provider configuration", "保存后写入当前供应商配置", "儲存後寫入當前供應商配置", "保存後、現在のプロバイダー設定を書き込む")}
        </span>
        <button
          className="btn primary"
          type="button"
          disabled={operationDisabled || selected.length === 0}
          onClick={save}
        >
          {saving ? copy("Saving…", "保存中…", "儲存中…", "保存中…") : copy("Save models", "保存模型", "儲存模型", "モデルを保存")}
        </button>
      </div>
    </div>
  );
}
