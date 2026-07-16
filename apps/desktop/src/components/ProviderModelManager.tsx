import { useMemo, useState } from "react";
import {
  ModelDiscoveryView,
  ProviderView,
  StateView,
  discoverProviderModels,
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
  const selectedSet = useMemo(() => new Set(selected), [selected]);
  const operationDisabled = disabled || refreshing || saving;

  const refresh = async () => {
    if (operationDisabled) return;
    setRefreshing(true);
    setError("");
    setStatus({ label: "正在获取…", tone: "loading" });
    try {
      const result = await discoverProviderModels(provider.name, provider.base_url, null);
      setModels((current) => mergeModels(current, result.models));
      setStatus(resultStatus(result));
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
