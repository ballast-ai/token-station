import { useMemo, useState } from "react";
import { useLocalizedCopy } from "./LanguageProvider";

export type CatalogTone = "idle" | "loading" | "live" | "cache" | "error";

export interface CatalogStatus {
  label: string;
  tone: CatalogTone;
  warning?: string | null;
}

interface ModelPickerProps {
  models: string[];
  selected: string[];
  status: CatalogStatus;
  onToggle: (model: string) => void;
  onAdd: (model: string) => void;
  onRefresh: () => void;
  refreshing: boolean;
  disabled?: boolean;
}

export default function ModelPicker({
  models,
  selected,
  status,
  onToggle,
  onAdd,
  onRefresh,
  refreshing,
  disabled = false,
}: ModelPickerProps) {
  const { copy } = useLocalizedCopy();
  const [query, setQuery] = useState("");
  const [customModel, setCustomModel] = useState("");
  const selectedSet = useMemo(() => new Set(selected), [selected]);
  const visible = useMemo(() => {
    const normalizedQuery = query.trim().toLocaleLowerCase();
    return [...new Set(models)]
      .filter((model) => !normalizedQuery || model.toLocaleLowerCase().includes(normalizedQuery));
  }, [models, query]);

  const addCustom = () => {
    const model = customModel.trim();
    if (!model || disabled) return;
    onAdd(model);
    setCustomModel("");
  };

  return (
    <div className="model-catalog">
      <div className="catalog-rail">
        <div className={`catalog-status ${status.tone}`} aria-live="polite">
          <span className="catalog-dot" />
          <span>{status.label}</span>
        </div>
        <button
          className="btn tiny quiet"
          type="button"
          onClick={onRefresh}
          disabled={disabled || refreshing}
        >
          {refreshing ? copy("Refreshing…", "同步中…") : copy("Refresh models", "刷新模型")}
        </button>
      </div>

      {status.warning && <div className="catalog-warning">{status.warning}</div>}

      {models.length > 12 && (
        <div className="model-search-wrap">
          <span className="search-mark">⌕</span>
          <input
            className="model-search"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            placeholder={copy(`Search ${models.length} models`, `搜索 ${models.length} 个模型`)}
            aria-label={copy("Search models", "搜索模型")}
            disabled={disabled}
          />
        </div>
      )}

      <div className={`model-chip-grid ${models.length > 12 ? "scrollable" : ""}`}>
        {visible.map((model) => (
          <button
            key={model}
            className={`model-chip ${selectedSet.has(model) ? "on" : ""}`}
            type="button"
            aria-pressed={selectedSet.has(model)}
            onClick={() => onToggle(model)}
            disabled={disabled}
          >
            <span className="model-check">{selectedSet.has(model) ? "✓" : "+"}</span>
            {model}
          </button>
        ))}
        {visible.length === 0 && (
          <div className="model-empty">{copy("No matching models", "没有匹配的模型")}</div>
        )}
      </div>

      <div className="custom-model-row">
        <input
          className="input grow"
          value={customModel}
          onChange={(event) => setCustomModel(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter") {
              event.preventDefault();
              addCustom();
            }
          }}
          placeholder={copy("Enter a model ID", "手动输入模型 ID")}
          aria-label={copy("Enter a model ID", "手动输入模型 ID")}
          disabled={disabled}
        />
        <button
          className="btn"
          type="button"
          onClick={addCustom}
          disabled={disabled || !customModel.trim()}
        >
          {copy("Add to list", "加入列表")}
        </button>
      </div>
      <div className="catalog-note">{copy(
        "Catalog requests do not consume inference tokens. Select models that support Chat Completions.",
        "目录不消耗推理 Token；请选择支持 Chat Completions 的模型。",
      )}</div>
    </div>
  );
}
