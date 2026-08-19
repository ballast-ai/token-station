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
import { useLocalizedCopy } from "./LanguageProvider";
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
  copy: (english: string, simplifiedChinese: string) => string,
) {
  if (stage.status === "skipped") return copy("Not run", "未执行");
  if (stage.duration_ms == null) return copy("Not timed", "未计时");
  return `${stage.timing_kind === "cumulative" ? "≤" : ""}${stage.duration_ms}ms`;
}

function costLabel(
  costMicros: number | null,
  copy: (english: string, simplifiedChinese: string) => string,
): string {
  return costMicros != null && costMicros > 0
    ? copy(
      `Estimated cost ${(costMicros / 1_000_000).toFixed(4)}`,
      `估算成本 ${(costMicros / 1_000_000).toFixed(4)}`,
    )
    : copy("Cost unknown", "成本未知");
}

const resultStatus = (
  result: ModelDiscoveryView,
  copy: (english: string, simplifiedChinese: string) => string,
  language: "en" | "zh-CN",
): CatalogStatus => {
  const warning = result.warning ? humanizeAppError(result.warning, language) : result.warning;
  if (result.source === "live") {
    return {
      label: copy(`Synced ${result.models.length}`, `已同步 ${result.models.length} 个`),
      tone: "live",
      warning,
    };
  }
  if (result.source === "cache") {
    return {
      label: copy(`Using cache · ${result.models.length}`, `使用缓存 · ${result.models.length} 个`),
      tone: "cache",
      warning,
    };
  }
  if (result.source === "preset") {
    return {
      label: copy("Using built-in presets", "使用内置预设"),
      tone: "cache",
      warning,
    };
  }
  return { label: copy("Fetch failed", "获取失败"), tone: "error", warning };
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
    label: copy(`Configured ${provider.models.length}`, `已配置 ${provider.models.length} 个`),
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
    "正在读取无正文用量…",
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
    verified: copy("Verified", "已验证"),
    declared: copy("Declared", "已声明"),
    unsupported: copy("Unsupported", "不支持"),
    unknown: copy("Unknown", "未知"),
  };
  const limitSourceLabel = {
    provider: copy("Provider API", "供应商接口"),
    builtin_preset: copy("Built-in preset (not live)", "内置预设（非实时值）"),
    operator: copy("Manual configuration", "手动配置"),
    heuristic: copy("Heuristic default", "启发式默认"),
  } as const;
  const catalogStateLabel = {
    active: copy("Active", "在售"),
    stale: copy("Cached, pending confirmation", "缓存待确认"),
    removed: copy("Removed", "已下架"),
  } as const;
  const catalogSourceLabel = {
    live: copy("Live catalog", "实时目录"),
    cache: copy("Local cache", "本地缓存"),
    configured: copy("Manual", "手工配置"),
  } as const;
  const probeLayerLabel = {
    network: copy("DNS / Network", "DNS / 网络"),
    http: "HTTP",
    auth: copy("Authentication", "鉴权"),
    model: copy("Model", "模型"),
    generation: copy("Generation", "生成"),
    stream: copy("Streaming", "流式"),
    tool: "Tool",
    json: "JSON Schema",
  } as const;
  const healthLabel: Record<ProviderHealth, string> = {
    untested: copy("Untested", "未测试"),
    healthy: copy("Healthy", "健康"),
    degraded: copy("Degraded capabilities", "能力退化"),
    unavailable: copy("Unavailable", "不可用"),
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
      "South 需要已验证的官方 OpenAI-compatible Provider 包。",
    ),
    api_dialect: copy(
      "South is available only for the translated API dialect.",
      "South 仅支持 translated API dialect。",
    ),
    egress: copy(
      "South currently requires direct egress.",
      "South 当前仅支持 direct egress。",
    ),
    auth: copy(
      "South requires Bearer credentials from the local store or an environment variable.",
      "South 需要来自本地存储或环境变量的 Bearer 凭据。",
    ),
  } as const;
  const headerAuthUnavailableCopy = {
    provider_package: copy(
      "South Header Auth requires the verified official package for this Provider dialect.",
      "South Header Auth 需要该 Provider dialect 对应的已验证官方包。",
    ),
    api_dialect: copy(
      "South Header Auth is available only for the translated API dialect.",
      "South Header Auth 仅支持 translated API dialect。",
    ),
    egress: copy(
      "South Header Auth currently requires direct egress.",
      "South Header Auth 当前仅支持 direct egress。",
    ),
    auth: copy(
      "South Header Auth requires credentials from the local store or an environment variable.",
      "South Header Auth 需要来自本地存储或环境变量的凭据。",
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
            `${aggregate.requests} 次请求 · ${aggregate.errors} 次错误 · P95 ${aggregate.p95_latency_ms}ms · ${aggregate.input_tokens + aggregate.output_tokens} tokens · ${costLabel(aggregate.cost_micros, copy)}`,
          )
          : copy("No request records", "暂无请求记录"));
      })
      .catch(() => setUsage(copy("Usage is temporarily unavailable", "用量暂不可读")));
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
        "上下文上限和最大输出 Token 必须是大于 0 的整数。",
      );
    } else if (output > context) {
      error = copy(
        "Maximum output tokens cannot exceed the context window.",
        "最大输出 Token 不能大于上下文上限。",
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
          ? copy("Model limits saved; restart the proxy to apply.", "已保存模型限制；重启代理后生效")
          : copy("Model limits saved.", "已保存模型限制"),
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
        copy(`Provider ${provider.name} details saved`, `${provider.name} 的基本信息已保存`),
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
        label: copy("Manual deployment", "手工填写 deployment"),
        tone: "idle",
        warning: humanizeAppError("model_catalog_azure_deployment_manual", language),
      });
      return;
    }
    setRefreshing(true);
    setStatus({ label: copy("Fetching…", "正在获取…"), tone: "loading" });
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
        copy(`Models for ${provider.name} refreshed`, `${provider.name} 的模型目录已刷新`),
        `provider-catalog:${provider.name}`,
      );
    } catch (caught) {
      const message = humanizeAppError(caught);
      setStatus({ label: copy("Fetch failed", "获取失败"), tone: "error", warning: null });
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
        label: copy(`Configured ${selected.length}`, `已配置 ${selected.length} 个`),
        tone: "idle",
      });
      showSuccess(
        copy(`Saved ${selected.length} models`, `已保存 ${selected.length} 个模型`),
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
          <strong>{copy("Provider details", "供应商详情")}</strong>
          <span>{usage}</span>
        </div>
        <div className="provider-health-actions">
          <div
            className={`provider-health-badge ${providerHealth(testResults)}`}
            aria-label={copy(
              `Provider health: ${healthLabel[providerHealth(testResults)]}`,
              `供应商健康状态：${healthLabel[providerHealth(testResults)]}`,
            )}
          >
            <i aria-hidden="true" />
            <span>{healthLabel[providerHealth(testResults)]}</span>
            {testedAtMs != null && (
              <small>{copy("Last tested", "最近测试")} {new Date(testedAtMs).toLocaleString(language === "zh-CN" ? "zh-CN" : "en-US")}</small>
            )}
          </div>
          <button className="btn tiny" type="button" disabled={operationDisabled} onClick={() => void runProviderTest()}>
            {testing ? copy("Testing…", "测试中…") : copy("Run layered test", "运行分层测试")}
          </button>
          <small className="provider-test-charge-warning">{copy(
            "This sends real Provider requests and may incur charges.",
            "该测试会向真实 Provider 发出请求，可能产生费用。",
          )}</small>
        </div>
      </div>
      <div className="provider-edit-fields">
        <input
          className="input mono"
          aria-label={copy("Edit base URL", "编辑 Base URL")}
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
            aria-label={copy("Edit credential source", "编辑凭据来源")}
          >
            <SelectValue />
          </SelectTrigger>
          <SelectContent position="popper" align="start">
            <SelectItem value="store">{copy("Local store (default)", "本地存储（默认）")}</SelectItem>
            <SelectItem value="env">{copy("Environment variable", "环境变量")}</SelectItem>
            <SelectItem value="file">{copy("Credential file", "凭据文件")}</SelectItem>
            <SelectItem value="none">{copy("No authentication", "无鉴权")}</SelectItem>
          </SelectContent>
        </Select>
        {credentialSource === "store" && (
          <input className="input mono" aria-label={copy("Update API key", "更新 API Key")} type="password" value={editKey} disabled={operationDisabled} placeholder={copy("Leave blank to keep the current key", "留空则保留现有 Key")} onChange={(event) => setEditKey(event.target.value)} />
        )}
        {(credentialSource === "env" || credentialSource === "file") && (
          <input
            className="input mono"
            aria-label={credentialSource === "env"
              ? copy("Environment variable name", "环境变量名")
              : copy("Absolute credential file path", "凭据文件绝对路径")}
            value={credentialReference}
            disabled={operationDisabled}
            placeholder={credentialSource === "env" ? "DEEPSEEK_API_KEY" : "/absolute/path/provider.key"}
            onChange={(event) => setCredentialReference(event.target.value)}
          />
        )}
        <button className="btn tiny" type="button" disabled={operationDisabled || !endpointPreview} onClick={() => void saveProviderDetails()}>
          {editing ? copy("Saving…", "保存中…") : copy("Save details", "保存基本信息")}
        </button>
      </div>
      <details
        className={`provider-runtime-advanced ${providerCall === "legacy" ? "stable" : "experimental"}`}
        open={runtimeOpen}
        onToggle={(event) => setRuntimeOpen(event.currentTarget.open)}
      >
        <summary>
          <span>{copy("Advanced runtime", "高级运行时")}</span>
          <small>
            {providerCall === "legacy"
              ? copy("Legacy active", "Legacy 已启用")
              : activeUnavailableReason
                ? copy(
                  "Experimental configured but unavailable",
                  "实验运行时已配置但不可用",
                )
                : copy("Experimental active", "实验运行时已启用")}
          </small>
        </summary>
        <div className="provider-runtime-panel">
          <p>{copy(
            "Choose how this provider sends requests. Legacy remains the stable default.",
            "选择此 Provider 发送请求的方式。Legacy 始终是稳定默认项。",
          )}</p>
          <fieldset className="provider-runtime-options">
            <legend>{copy("Provider runtime", "Provider 运行时")}</legend>
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
                <small>{copy("Stable default · broad compatibility", "稳定默认 · 兼容性最广")}</small>
              </span>
              <i>{copy("Stable", "稳定")}</i>
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
                <strong>{copy("South buffered only", "South 仅非流式")}</strong>
                <small>{copy(
                  "South handles eligible buffered requests. Streams remain on Legacy.",
                  "符合条件的非流式请求使用 South。流式请求仍使用 Legacy。",
                )}</small>
              </span>
              <i>{copy("Experimental", "实验")}</i>
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
                <strong>{copy("South buffered + streaming", "South 非流式 + 流式")}</strong>
                <small>{copy(
                  "South for eligible buffered and streaming requests.",
                  "符合条件的非流式和流式请求均使用 South。",
                )}</small>
              </span>
              <i>{copy("Experimental", "实验")}</i>
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
                  "South 非流式 + 流式 + Header Auth",
                )}</strong>
                <small>{copy(
                  "Adds fixed api-key Header Auth for eligible Azure OpenAI v1 calls.",
                  "为符合条件的 Azure OpenAI v1 调用启用固定 api-key Header Auth。",
                )}</small>
              </span>
              <i>{copy("Experimental", "实验")}</i>
            </label>
          </fieldset>
          <p className={`provider-runtime-note ${providerCall === "legacy" ? "" : "warning"}`}>
            {activeUnavailableReason
              ? activeUnavailableCopy[activeUnavailableReason]
              : providerCall === "legacy"
                ? copy(
                  "South is eligible for this provider. Enable it only for a controlled canary.",
                  "此 Provider 符合 South 条件。仅在受控 canary 中启用。",
                )
                : usesHeaderAuthEngine
                  ? copy(
                    "Eligible calls use South. Azure OpenAI v1 uses the fixed api-key header. South attempts never replay through Legacy.",
                    "符合条件的调用使用 South。Azure OpenAI v1 使用固定 api-key header。South 尝试不会通过 Legacy 重放。",
                  )
                : copy(
                  "South never replays an attempt through Legacy once South execution begins.",
                  "South 一旦开始执行，就不会再通过 Legacy 重放该次尝试。",
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
        <div className="provider-endpoint-list" aria-label={copy("Final provider URLs", "供应商最终地址")}>
          <code>{endpointPreview.chat}</code>
          <code>{endpointPreview.responses}</code>
          <code>{endpointPreview.messages}</code>
        </div>
      )}
      {testResults.length > 0 && (
        <div className="provider-test-results" aria-label={copy("Provider layered test results", "供应商分层测试结果")}>
          <p>{copy(
            "The five base layers show cumulative timing (≤) for one live generation probe; capability layers show their own request timing.",
            "基础五层显示同一次真实生成探测的累计耗时（≤）；能力层显示各自真实请求耗时。",
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
        <div className="catalog-diff" aria-label={copy("Model catalog changes", "模型目录变化")}>
          {diff.added.length > 0 && (
            <span className="added">{copy(
              `Added: ${diff.added.join(", ")}`,
              `新增：${diff.added.join("、")}`,
            )}</span>
          )}
          {diff.removed.length > 0 && (
            <span className="removed">{copy(
              `Removed: ${diff.removed.join(", ")} (references retained)`,
              `下架：${diff.removed.join("、")}（仍保留引用）`,
            )}</span>
          )}
        </div>
      )}
      {modelRows.length > 0 && (
        <div className="model-ledger" aria-label={copy("Model catalog and capabilities", "模型目录与能力")}>
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
                  [copy("Tools", "工具"), row.cap.tool],
                  [copy("Vision", "视觉"), row.cap.vision],
                  ["JSON", row.cap.json_schema],
                ] as const).map(([label, state]) => (
                  label === copy("Vision", "视觉") ? (
                    <button
                      aria-label={state === "verified"
                        ? copy(`${row.model} vision capability verified`, `${row.model} 视觉能力已验证`)
                        : state === "declared"
                          ? copy(`Mark ${row.model} as not supporting vision`, `将 ${row.model} 标记为不支持视觉`)
                          : copy(`Declare vision support for ${row.model}`, `为 ${row.model} 声明视觉支持`)}
                      aria-pressed={state === "verified" || state === "declared"}
                      className={`capability-tag capability-tag-button ${state}`}
                      disabled={operationDisabled || state === "verified"}
                      key={label}
                      onClick={() => void toggleVision(row.model, state)}
                      type="button"
                    >
                      {label} · {capabilitySaving === row.model ? copy("Saving…", "保存中…") : capabilityLabel[state]}
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
                  {copy("Limit source", "限制来源")} · {limitSourceLabel[row.cap.context_window_source]}
                </p>
              )}
              {row.configured && row.cap.context_window_source
                && row.cap.max_output_tokens_source
                && row.cap.context_window_source !== row.cap.max_output_tokens_source && (
                <p className="model-limit-source">
                  {copy("Context", "上下文")} · {limitSourceLabel[row.cap.context_window_source]}
                  {" · "}
                  {copy("Output", "输出")} · {limitSourceLabel[row.cap.max_output_tokens_source]}
                </p>
              )}
              {row.configured && <div className="model-limit-editor">
                <label>
                  <span>{copy("Context", "上下文上限")}</span>
                  <input
                    aria-label={copy(`${row.model} context window`, `${row.model} 上下文上限`)}
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
                  <span>{copy("Maximum output", "最大输出 Token")}</span>
                  <input
                    aria-label={copy(`${row.model} maximum output tokens`, `${row.model} 最大输出 Token`)}
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
                  aria-label={copy(`Save ${row.model} model limits`, `保存 ${row.model} 模型限制`)}
                  className="btn tiny"
                  disabled={operationDisabled}
                  onClick={() => void saveLimits(row.model)}
                  type="button"
                >
                  {limitSaving === row.model ? copy("Saving…", "保存中…") : copy("Save limits", "保存限制")}
                </button>
              </div>}
              {row.configured && !row.cap.max_output_tokens && (
                <p className="model-limit-warning">
                  {copy(
                    "This model's metadata is missing a maximum output limit.",
                    "该模型元数据缺少最大输出上限",
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
            ? copy("Proxy running · Restart after saving to apply", "代理运行中 · 保存后重启代理生效")
            : copy("Save to write the current provider configuration", "保存后写入当前供应商配置")}
        </span>
        <button
          className="btn primary"
          type="button"
          disabled={operationDisabled || selected.length === 0}
          onClick={save}
        >
          {saving ? copy("Saving…", "保存中…") : copy("Save models", "保存模型")}
        </button>
      </div>
    </div>
  );
}
