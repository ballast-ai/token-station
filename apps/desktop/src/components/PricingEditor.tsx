import { useEffect, useState } from "react";
import {
  ModelPriceView,
  ModelPriceSuggestionView,
  PriceTableView,
  getPriceTable,
  removeModelPrice,
  setModelPrice,
  suggestModelPrice,
} from "../api";
import { useLocalizedCopy } from "./LanguageProvider";

function displayRate(rate: number | null): string {
  return rate == null ? "" : String(rate / 1_000_000);
}

function rateMicros(raw: string, invalidMessage: string): number {
  const value = Number(raw);
  const micros = Math.round(value * 1_000_000);
  if (!/^\d+(?:\.\d{1,6})?$/.test(raw)
      || !Number.isFinite(value)
      || value < 0
      || value > 9_000_000_000
      || !Number.isSafeInteger(micros)) {
    throw new Error(invalidMessage);
  }
  return micros;
}

export default function PricingEditor() {
  const { copy } = useLocalizedCopy();
  const [table, setTable] = useState<PriceTableView | null>(null);
  const [model, setModel] = useState("");
  const [input, setInput] = useState("0");
  const [output, setOutput] = useState("0");
  const [cacheRead, setCacheRead] = useState("0");
  const [cacheWrite, setCacheWrite] = useState("0");
  const [reasoning, setReasoning] = useState("");
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  // A dirty price belongs to one model only. Reusing it after the model ID
  // changes can silently save one model's price under another model.
  const [touchedModel, setTouchedModel] = useState<string | null>(null);
  const [lookupRequestedFor, setLookupRequestedFor] = useState<string | null>(null);
  const [suggestion, setSuggestion] = useState<ModelPriceSuggestionView | null>(null);

  useEffect(() => {
    getPriceTable().then(setTable).catch((value) => setError(String(value)));
  }, []);

  useEffect(() => {
    const requestedModel = model.trim();
    setSuggestion(null);
    if (!table
        || requestedModel.length === 0
        || requestedModel.length > 256
        || table.models[requestedModel]
        || lookupRequestedFor !== requestedModel
        || touchedModel === requestedModel) {
      return;
    }
    let cancelled = false;
    const timer = window.setTimeout(() => {
      suggestModelPrice(null, requestedModel)
        .then((value) => {
          if (cancelled || !value) return;
          setInput(displayRate(value.input_per_mtok));
          setOutput(displayRate(value.output_per_mtok));
          setCacheRead(displayRate(value.cache_read_per_mtok));
          setCacheWrite(displayRate(value.cache_write_per_mtok));
          setReasoning(displayRate(value.reasoning_per_mtok));
          setSuggestion(value);
        })
        // Suggestions are optional. Manual entry remains available offline.
        .catch(() => undefined);
    }, 350);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [lookupRequestedFor, model, table, touchedModel]);

  const edit = (name: string, price: ModelPriceView) => {
    setModel(name);
    setInput(displayRate(price.input_per_mtok));
    setOutput(displayRate(price.output_per_mtok));
    setCacheRead(displayRate(price.cache_read_per_mtok));
    setCacheWrite(displayRate(price.cache_write_per_mtok));
    setReasoning(displayRate(price.reasoning_per_mtok));
    setError("");
    setNotice("");
    setTouchedModel(name);
    setSuggestion(null);
  };

  const save = async () => {
    setError("");
    setNotice("");
    if (!table) return;
    if (!model || model.trim() !== model || model.length > 256) {
      setError(copy(
        "Model ID must be 1–256 characters with no leading or trailing spaces.",
        "模型 ID 必须是 1–256 个字符，且首尾不能有空格。",
      ));
      return;
    }
    try {
      const price: ModelPriceView = {
        input_per_mtok: rateMicros(input, copy(
          "Input price must be a valid amount from 0 to 9 billion with at most 6 decimal places.",
          "输入价格必须是 0 到 90 亿之间、最多 6 位小数的有效金额。",
        )),
        output_per_mtok: rateMicros(output, copy(
          "Output price must be a valid amount from 0 to 9 billion with at most 6 decimal places.",
          "输出价格必须是 0 到 90 亿之间、最多 6 位小数的有效金额。",
        )),
        cache_read_per_mtok: rateMicros(cacheRead, copy(
          "Cache read price must be a valid amount from 0 to 9 billion with at most 6 decimal places.",
          "缓存读取价格必须是 0 到 90 亿之间、最多 6 位小数的有效金额。",
        )),
        cache_write_per_mtok: rateMicros(cacheWrite, copy(
          "Cache write price must be a valid amount from 0 to 9 billion with at most 6 decimal places.",
          "缓存写入价格必须是 0 到 90 亿之间、最多 6 位小数的有效金额。",
        )),
        reasoning_per_mtok: reasoning === "" ? null : rateMicros(reasoning, copy(
          "Reasoning price must be a valid amount from 0 to 9 billion with at most 6 decimal places.",
          "推理价格必须是 0 到 90 亿之间、最多 6 位小数的有效金额。",
        )),
      };
      const next = await setModelPrice(model, price, table.version);
      setTable(next);
      setNotice(copy(
        `Created price v${next.version}. Reapply the configuration to update the running proxy.`,
        `已生成 price v${next.version}；正在运行的代理需重新应用配置。`,
      ));
    } catch (value) {
      setError(String(value));
    }
  };

  const remove = async (name: string) => {
    if (!table) return;
    setError("");
    setNotice("");
    try {
      const next = await removeModelPrice(name, table.version);
      setTable(next);
      if (model === name) {
        setModel("");
        setInput("0");
        setOutput("0");
        setCacheRead("0");
        setCacheWrite("0");
        setReasoning("");
        setTouchedModel(null);
        setLookupRequestedFor(null);
        setSuggestion(null);
      }
      setNotice(copy(
        `Created price v${next.version}. Historical receipts keep their original cost.`,
        `已生成 price v${next.version}；历史回执保持原成本。`,
      ));
    } catch (value) {
      setError(String(value));
    }
  };

  return (
    <div className="pricing-section">
      <div className="budget-title-row">
        <div>
          <h3>{copy("Versioned model pricing", "版本化模型定价")}</h3>
          <p>{copy(
            "Amounts use the account currency per 1M tokens. Each price change creates a version; historical receipts are not recalculated.",
            "金额单位：账户货币 / 1M tokens。每次实际改价生成新版本，历史回执不会重算。",
          )}</p>
        </div>
        <span className="price-version">price v{table?.version ?? "—"}</span>
      </div>

      {table && Object.keys(table.models).length === 0 && (
        <div className="empty sm">{copy(
          "No model prices configured. Cost remains unknown for models without a price.",
          "尚未配置模型价格；缺失模型的成本保持未知。",
        )}</div>
      )}
      {table && Object.keys(table.models).length > 0 && (
        <table className="grid-table price-table">
          <thead>
            <tr>
              <th>{copy("Model", "模型")}</th>
              <th>{copy("Input", "输入")}</th>
              <th>{copy("Output", "输出")}</th>
              <th>{copy("Cache read", "缓存读")}</th>
              <th>{copy("Cache write", "缓存写")}</th>
              <th>{copy("Reasoning", "推理")}</th>
              <th>{copy("Actions", "操作")}</th>
            </tr>
          </thead>
          <tbody>
            {Object.entries(table.models).map(([name, price]) => (
              <tr key={name}>
                <td className="mono">{name}</td>
                <td>{displayRate(price.input_per_mtok)}</td>
                <td>{displayRate(price.output_per_mtok)}</td>
                <td>{displayRate(price.cache_read_per_mtok)}</td>
                <td>{displayRate(price.cache_write_per_mtok)}</td>
                <td>{price.reasoning_per_mtok == null
                  ? copy("Same as output", "跟随输出")
                  : displayRate(price.reasoning_per_mtok)}</td>
                <td className="price-actions">
                  <button className="btn tiny" aria-label={copy(`Edit ${name}`, `编辑 ${name}`)} onClick={() => edit(name, price)}>
                    {copy("Edit", "编辑")}
                  </button>
                  <button className="btn tiny danger" aria-label={copy(`Delete ${name}`, `删除 ${name}`)} onClick={() => remove(name)}>
                    {copy("Delete", "删除")}
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}

      <div className="price-form">
        <label className="field-label price-model-field">
          {copy("Model ID", "模型 ID")}
          <input
            aria-label={copy("Model ID", "模型 ID")}
            className="input"
            value={model}
            onChange={(event) => {
              // Changing the identity invalidates both an automatic suggestion
              // and any manually entered amount for the previous model.
              setInput("0");
              setOutput("0");
              setCacheRead("0");
              setCacheWrite("0");
              setReasoning("");
              setSuggestion(null);
              setTouchedModel(null);
              setLookupRequestedFor(null);
              setModel(event.target.value);
            }}
          />
        </label>
        {[
          [copy("Input price", "输入价格"), input, setInput, false],
          [copy("Output price", "输出价格"), output, setOutput, false],
          [copy("Cache read price", "缓存读取价格"), cacheRead, setCacheRead, false],
          [copy("Cache write price", "缓存写入价格"), cacheWrite, setCacheWrite, false],
          [copy("Reasoning price", "推理价格"), reasoning, setReasoning, true],
        ].map(([label, value, setter, optional]) => (
          <label className="field-label" key={label as string}>
            {label as string}{optional ? copy(" (empty uses output price)", "（空=跟随输出）") : ""}
            <input
              aria-label={label as string}
              className="input"
              type="number"
              min="0"
              step="0.000001"
              value={value as string}
              onChange={(event) => {
                setTouchedModel(model.trim());
                setSuggestion(null);
                (setter as (value: string) => void)(event.target.value);
              }}
            />
          </label>
        ))}
        <div className="price-save-action">
          <button
            className="btn"
            disabled={!table || model.trim().length === 0 || table.models[model.trim()] != null}
            onClick={() => setLookupRequestedFor(model.trim())}
          >
            {copy("Look up public price", "查询公开价格")}
          </button>
          <button className="btn primary" disabled={!table} onClick={save}>
            {copy("Save new version", "保存新版本")}
          </button>
        </div>
      </div>
      {suggestion && (
        <div className="banner">
          {copy(
            `Prefilled from the public USD price published by ${suggestion.source} for ${suggestion.provider_name} / ${suggestion.display_name}. Review it before saving a new version.`,
            `已按 ${suggestion.source} 的 ${suggestion.provider_name} / ${suggestion.display_name} 公开美元标价预填；尚未保存，请核对后生成新版本。`,
          )}
        </div>
      )}
      {error && <div className="banner err">{error}</div>}
      {notice && <div className="banner ok">{notice}</div>}
    </div>
  );
}
