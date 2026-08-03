import { useEffect, useMemo, useState } from "react";
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
  ProviderTestResult,
  setProviderModelVision,
  testProvider,
  updateProviderModels,
} from "../api";
import ModelPicker, { CatalogStatus } from "./ModelPicker";
import { useLocalizedCopy } from "./LanguageProvider";

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
});

type ProviderHealth = "untested" | "healthy" | "degraded" | "unavailable";

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
): CatalogStatus => {
  if (result.source === "live") {
    return {
      label: copy(`Synced ${result.models.length}`, `已同步 ${result.models.length} 个`),
      tone: "live",
      warning: result.warning,
    };
  }
  if (result.source === "cache") {
    return {
      label: copy(`Using cache · ${result.models.length}`, `使用缓存 · ${result.models.length} 个`),
      tone: "cache",
      warning: result.warning,
    };
  }
  return { label: copy("Fetch failed", "获取失败"), tone: "error", warning: result.warning };
};

export default function ProviderModelManager({
  provider,
  serveRunning,
  disabled = false,
  onSaved,
}: ProviderModelManagerProps) {
  const { copy, language } = useLocalizedCopy();
  const [models, setModels] = useState(provider.models);
  const [selected, setSelected] = useState(provider.models);
  const [status, setStatus] = useState<CatalogStatus>({
    label: copy(`Configured ${provider.models.length}`, `已配置 ${provider.models.length} 个`),
    tone: "idle",
  });
  const [refreshing, setRefreshing] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const [catalog, setCatalog] = useState<CatalogModelView[]>(provider.catalog ?? []);
  const [diff, setDiff] = useState<{ added: string[]; removed: string[] } | null>(null);
  const [endpointPreview, setEndpointPreview] = useState<ProviderEndpointPreview | null>(null);
  const [testResults, setTestResults] = useState<ProviderTestResult[]>([]);
  const [testedAtMs, setTestedAtMs] = useState<number | null>(null);
  const [testing, setTesting] = useState(false);
  const [capabilitySaving, setCapabilitySaving] = useState<string | null>(null);
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
      catalog: catalogByModel.get(model) ?? null,
      cap: capByModel.get(model) ?? unknownCapabilities(model),
    }));
  }, [catalog, capabilities]);
  const operationDisabled = disabled || refreshing || saving || testing || editing || capabilitySaving !== null;
  const capabilityLabel: Record<CapabilityState, string> = {
    verified: copy("Verified", "已验证"),
    declared: copy("Declared", "已声明"),
    unsupported: copy("Unsupported", "不支持"),
    unknown: copy("Unknown", "未知"),
  };
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

  useEffect(() => {
    void previewProviderEndpoints(editBaseUrl).then(setEndpointPreview).catch(() => setEndpointPreview(null));
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

  const saveProviderDetails = async () => {
    if (operationDisabled || !endpointPreview) return;
    setEditing(true);
    setError("");
    try {
      onSaved(await editProvider(
        provider.name,
        editBaseUrl,
        credentialSource === "store" ? editKey.trim() || null : null,
        credentialSource,
        credentialSource === "env" || credentialSource === "file"
          ? credentialReference.trim()
          : null,
      ));
      setEditKey("");
    } catch (caught) {
      setError(String(caught));
    } finally {
      setEditing(false);
    }
  };

  const runProviderTest = async () => {
    if (operationDisabled) return;
    setTesting(true);
    setError("");
    try {
      const results = await testProvider(provider.name);
      setTestResults(results);
      setTestedAtMs(Date.now());
    } catch (caught) {
      setError(String(caught));
    } finally {
      setTesting(false);
    }
  };

  const toggleVision = async (model: string, state: CapabilityState) => {
    if (operationDisabled || state === "verified") return;
    setCapabilitySaving(model);
    setError("");
    try {
      onSaved(await setProviderModelVision(provider.name, model, state !== "declared"));
    } catch (caught) {
      setError(String(caught));
    } finally {
      setCapabilitySaving(null);
    }
  };

  const refresh = async () => {
    if (operationDisabled) return;
    setRefreshing(true);
    setError("");
    setStatus({ label: copy("Fetching…", "正在获取…"), tone: "loading" });
    try {
      const result = await discoverProviderModels(provider.name, provider.base_url, null);
      setModels((current) => mergeModels(current, result.models));
      setCatalog(result.catalog ?? []);
      setDiff({ added: result.added ?? [], removed: result.removed ?? [] });
      setStatus(resultStatus(result, copy));
      if (result.capabilities_updated) {
        onSaved(await getState());
      }
    } catch (caught) {
      setStatus({ label: copy("Fetch failed", "获取失败"), tone: "error", warning: String(caught) });
    } finally {
      setRefreshing(false);
    }
  };

  const save = async () => {
    if (operationDisabled) return;
    setSaving(true);
    setError("");
    try {
      const next = await updateProviderModels(provider.name, selected);
      onSaved(next);
      setStatus({ label: copy(`Saved ${selected.length}`, `已保存 ${selected.length} 个`), tone: "live" });
    } catch (caught) {
      setError(String(caught));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="provider-model-manager">
      <div className="provider-detail-summary">
        <div>
          <strong>{copy("Provider details", "Provider 详情")}</strong>
          <span>{usage}</span>
        </div>
        <div className="provider-health-actions">
          <div
            className={`provider-health-badge ${providerHealth(testResults)}`}
            aria-label={copy(
              `Provider health: ${healthLabel[providerHealth(testResults)]}`,
              `Provider 健康状态：${healthLabel[providerHealth(testResults)]}`,
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
        </div>
      </div>
      <div className="provider-edit-fields">
        <input className="input mono" aria-label={copy("Edit base URL", "编辑 Base URL")} value={editBaseUrl} disabled={operationDisabled} onChange={(event) => setEditBaseUrl(event.target.value)} />
        <select
          className="input"
          aria-label={copy("Edit credential source", "编辑凭据来源")}
          value={credentialSource}
          disabled={operationDisabled}
          onChange={(event) => {
            setCredentialSource(event.target.value as typeof credentialSource);
            setEditKey("");
            setCredentialReference("");
          }}
        >
          <option value="store">{copy("Local store (default)", "本地存储（默认）")}</option>
          <option value="env">{copy("Environment variable", "环境变量")}</option>
          <option value="file">{copy("Credential file", "凭据文件")}</option>
          <option value="none">{copy("No authentication", "无鉴权")}</option>
        </select>
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
      {endpointPreview && (
        <div className="provider-endpoint-list" aria-label={copy("Final provider URLs", "Provider 最终 URL")}>
          <code>{endpointPreview.chat}</code>
          <code>{endpointPreview.responses}</code>
          <code>{endpointPreview.messages}</code>
        </div>
      )}
      {testResults.length > 0 && (
        <div className="provider-test-results" aria-label={copy("Provider layered test results", "Provider 分层测试结果")}>
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
      {error && <div className="manager-error">{error}</div>}
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
