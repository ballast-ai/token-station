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

interface ProviderModelManagerProps {
  provider: ProviderView;
  serveRunning: boolean;
  disabled?: boolean;
  onSaved: (state: StateView) => void;
}

const mergeModels = (...groups: string[][]) => [...new Set(groups.flat())];

const capabilityLabel: Record<CapabilityState, string> = {
  verified: "已验证",
  declared: "已声明",
  unsupported: "不支持",
  unknown: "未知",
};

const unknownCapabilities = (model: string): ModelCapabilityView => ({
  model,
  tool: "unknown",
  vision: "unknown",
  json_schema: "unknown",
});

const catalogStateLabel = {
  active: "在售",
  stale: "缓存待确认",
  removed: "已下架",
} as const;

const catalogSourceLabel = {
  live: "实时目录",
  cache: "本地缓存",
  configured: "手工配置",
} as const;

const probeLayerLabel = {
  network: "DNS / 网络",
  http: "HTTP",
  auth: "鉴权",
  model: "模型",
  generation: "生成",
  stream: "流式",
  tool: "Tool",
  json: "JSON Schema",
} as const;

type ProviderHealth = "untested" | "healthy" | "degraded" | "unavailable";

const healthLabel: Record<ProviderHealth, string> = {
  untested: "未测试",
  healthy: "健康",
  degraded: "能力退化",
  unavailable: "不可用",
};

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

function stageTiming(stage: ProviderTestResult["stages"][number]) {
  if (stage.status === "skipped") return "未执行";
  if (stage.duration_ms == null) return "未计时";
  return `${stage.timing_kind === "cumulative" ? "≤" : ""}${stage.duration_ms}ms`;
}

function costLabel(costMicros: number | null): string {
  return costMicros != null && costMicros > 0
    ? `估算成本 ${(costMicros / 1_000_000).toFixed(4)}`
    : "成本未知";
}

const resultStatus = (result: ModelDiscoveryView): CatalogStatus => {
  if (result.source === "live") {
    return {
      label: `已同步 ${result.models.length} 个`,
      tone: "live",
      warning: result.warning,
    };
  }
  if (result.source === "cache") {
    return {
      label: `使用缓存 · ${result.models.length} 个`,
      tone: "cache",
      warning: result.warning,
    };
  }
  return { label: "获取失败", tone: "error", warning: result.warning };
};

export default function ProviderModelManager({
  provider,
  serveRunning,
  disabled = false,
  onSaved,
}: ProviderModelManagerProps) {
  const [models, setModels] = useState(provider.models);
  const [selected, setSelected] = useState(provider.models);
  const [status, setStatus] = useState<CatalogStatus>({
    label: `已配置 ${provider.models.length} 个`,
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
  const [usage, setUsage] = useState<string>("正在读取无正文用量…");
  const [editBaseUrl, setEditBaseUrl] = useState(provider.base_url);
  const [editKey, setEditKey] = useState("");
  const [editing, setEditing] = useState(false);
  const selectedSet = useMemo(() => new Set(selected), [selected]);
  const capabilities = useMemo(() => {
    const byModel = new Map((provider.model_capabilities ?? []).map((item) => [item.model, item]));
    return provider.models.map((model) => byModel.get(model) ?? unknownCapabilities(model));
  }, [provider.model_capabilities, provider.models]);
  const operationDisabled = disabled || refreshing || saving || testing || editing || capabilitySaving !== null;

  useEffect(() => {
    void previewProviderEndpoints(editBaseUrl).then(setEndpointPreview).catch(() => setEndpointPreview(null));
  }, [editBaseUrl]);

  useEffect(() => {
    void getStats("all", "upstream")
      .then((view) => {
        const aggregate = view.groups.find(([name]) => name === provider.name)?.[1];
        setUsage(aggregate
          ? `${aggregate.requests} 次请求 · ${aggregate.errors} 次错误 · P95 ${aggregate.p95_latency_ms}ms · ${aggregate.input_tokens + aggregate.output_tokens} tokens · ${costLabel(aggregate.cost_micros)}`
          : "暂无请求记录");
      })
      .catch(() => setUsage("用量暂不可读"));
  }, [provider.name]);

  const saveProviderDetails = async () => {
    if (operationDisabled || !endpointPreview) return;
    setEditing(true);
    setError("");
    try {
      onSaved(await editProvider(provider.name, editBaseUrl, editKey.trim() || null));
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
    setStatus({ label: "正在获取…", tone: "loading" });
    try {
      const result = await discoverProviderModels(provider.name, provider.base_url, null);
      setModels((current) => mergeModels(current, result.models));
      setCatalog(result.catalog ?? []);
      setDiff({ added: result.added ?? [], removed: result.removed ?? [] });
      setStatus(resultStatus(result));
      if (result.capabilities_updated) {
        onSaved(await getState());
      }
    } catch (caught) {
      setStatus({ label: "获取失败", tone: "error", warning: String(caught) });
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
      setStatus({ label: `已保存 ${selected.length} 个`, tone: "live" });
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
          <strong>Provider 详情</strong>
          <span>{usage}</span>
        </div>
        <div className="provider-health-actions">
          <div
            className={`provider-health-badge ${providerHealth(testResults)}`}
            aria-label={`Provider 健康状态：${healthLabel[providerHealth(testResults)]}`}
          >
            <i aria-hidden="true" />
            <span>{healthLabel[providerHealth(testResults)]}</span>
            {testedAtMs != null && <small>最近测试 {new Date(testedAtMs).toLocaleString()}</small>}
          </div>
          <button className="btn tiny" type="button" disabled={operationDisabled} onClick={() => void runProviderTest()}>
            {testing ? "测试中…" : "运行分层测试"}
          </button>
        </div>
      </div>
      <div className="provider-edit-fields">
        <input className="input mono" aria-label="编辑 Base URL" value={editBaseUrl} disabled={operationDisabled} onChange={(event) => setEditBaseUrl(event.target.value)} />
        <input className="input mono" aria-label="更新 API Key" type="password" value={editKey} disabled={operationDisabled} placeholder="留空则保留现有 Key" onChange={(event) => setEditKey(event.target.value)} />
        <button className="btn tiny" type="button" disabled={operationDisabled || !endpointPreview} onClick={() => void saveProviderDetails()}>
          {editing ? "保存中…" : "保存基本信息"}
        </button>
      </div>
      {endpointPreview && (
        <div className="provider-endpoint-list" aria-label="Provider 最终 URL">
          <code>{endpointPreview.chat}</code>
          <code>{endpointPreview.responses}</code>
          <code>{endpointPreview.messages}</code>
        </div>
      )}
      {testResults.length > 0 && (
        <div className="provider-test-results" aria-label="Provider 分层测试结果">
          <p>基础五层显示同一次真实生成探测的累计耗时（≤）；能力层显示各自真实请求耗时。</p>
          {testResults.map((result) => (
            <div key={result.model}>
              <strong>{result.model}</strong>
              {result.stages.map((stage) => (
                <span className={stage.status} key={stage.layer} title={stage.detail}>
                  {probeLayerLabel[stage.layer]} · {stage.status} · {stageTiming(stage)}
                </span>
              ))}
            </div>
          ))}
        </div>
      )}
      {diff && (diff.added.length > 0 || diff.removed.length > 0) && (
        <div className="catalog-diff" aria-label="模型目录变化">
          {diff.added.length > 0 && <span className="added">新增：{diff.added.join("、")}</span>}
          {diff.removed.length > 0 && <span className="removed">下架：{diff.removed.join("、")}（仍保留引用）</span>}
        </div>
      )}
      {catalog.length > 0 && (
        <div className="catalog-ledger" aria-label="可信模型目录">
          {catalog.map((model) => (
            <div className={`catalog-ledger-row ${model.catalog_state}`} key={model.model}>
              <code>{model.model}</code>
              <span>{catalogStateLabel[model.catalog_state]}</span>
              <span>{catalogSourceLabel[model.source]}</span>
              <span>{model.last_seen_ms ? `最后见到 ${new Date(model.last_seen_ms).toLocaleString()}` : "尚无实时记录"}</span>
            </div>
          ))}
        </div>
      )}
      <div className="capability-table" aria-label="模型能力状态">
        {capabilities.map((capability) => (
          <div className="capability-row" key={capability.model}>
            <code>{capability.model}</code>
            {([
              ["工具", capability.tool],
              ["视觉", capability.vision],
              ["JSON", capability.json_schema],
            ] as const).map(([label, state]) => (
              label === "视觉" ? (
                <button
                  aria-label={state === "verified"
                    ? `${capability.model} 视觉能力已验证`
                    : state === "declared"
                      ? `将 ${capability.model} 标记为不支持视觉`
                      : `为 ${capability.model} 声明视觉支持`}
                  aria-pressed={state === "verified" || state === "declared"}
                  className={`capability-tag capability-tag-button ${state}`}
                  disabled={operationDisabled || state === "verified"}
                  key={label}
                  onClick={() => void toggleVision(capability.model, state)}
                  type="button"
                >
                  {label} · {capabilitySaving === capability.model ? "保存中…" : capabilityLabel[state]}
                </button>
              ) : (
                <span className={`capability-tag ${state}`} key={label}>
                  {label} · {capabilityLabel[state]}
                </span>
              )
            ))}
          </div>
        ))}
      </div>
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
          {serveRunning ? "代理运行中 · 保存后重启代理生效" : "保存后写入当前供应商配置"}
        </span>
        <button
          className="btn primary"
          type="button"
          disabled={operationDisabled || selected.length === 0}
          onClick={save}
        >
          {saving ? "保存中…" : "保存模型"}
        </button>
      </div>
    </div>
  );
}
