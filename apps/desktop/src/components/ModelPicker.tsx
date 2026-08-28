import { useMemo, useState } from "react";
import { useLocalizedCopy } from "./LanguageProvider";
import { Input } from "./ui/input";

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
      .sort((left, right) => left.localeCompare(right))
      .filter((model) => !normalizedQuery || model.toLocaleLowerCase().includes(normalizedQuery));
  }, [models, query]);

  const toggle = (model: string) => onToggle(model);

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
          {refreshing ? copy("Refreshing…", "同步中…", "同步中…", "同期中…") : copy("Refresh models", "刷新模型", "重新整理模型", "モデルを再取得")}
        </button>
      </div>

      {status.warning && <div className="catalog-warning">{status.warning}</div>}

      {models.length > 12 && (
        <div className="model-search-wrap">
          <span className="search-mark">⌕</span>
          <Input
            className="model-search"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            placeholder={copy(`Search ${models.length} models`, `搜索 ${models.length} 个模型`, `搜尋 ${models.length} 個模型`, `${models.length} 個モデルを検索`)}
            aria-label={copy("Search models", "搜索模型", "搜尋模型", "モデルを検索")}
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
            onClick={() => toggle(model)}
            disabled={disabled}
          >
            <span className="model-check">{selectedSet.has(model) ? "✓" : "+"}</span>
            {model}
          </button>
        ))}
        {visible.length === 0 && (
          <div className="model-empty">{copy("No matching models", "没有匹配的模型", "沒有匹配的模型", "一致するモデルがありません")}</div>
        )}
      </div>

      <div className="custom-model-row">
        <Input
          className="input grow"
          value={customModel}
          onChange={(event) => setCustomModel(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter") {
              event.preventDefault();
              addCustom();
            }
          }}
          placeholder={copy("Enter a model ID", "手动输入模型 ID", "手動輸入模型 ID", "モデル ID を手動で入力")}
          aria-label={copy("Enter a model ID", "手动输入模型 ID", "手動輸入模型 ID", "モデル ID を手動で入力")}
          disabled={disabled}
        />
        <button
          className="btn"
          type="button"
          onClick={addCustom}
          disabled={disabled || !customModel.trim()}
        >
          {copy("Add to list", "加入列表", "加入清單", "リストに追加")}
        </button>
      </div>
      <div className="catalog-note">{copy(
        "Catalog requests do not consume inference tokens. Select models that support Chat Completions.",
        "目录不消耗推理 Token；请选择支持 Chat Completions 的模型。", "目錄請求不消耗推理 Token；請選擇支援 Chat Completions 的模型。", "カタログリクエストは推論トークンを消費しません。Chat Completions をサポートするモデルを選択してください。"
      )}</div>
    </div>
  );
}
