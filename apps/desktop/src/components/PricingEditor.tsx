import { useEffect, useState } from "react";
import {
  ModelPriceView,
  PriceTableView,
  getPriceTable,
  removeModelPrice,
  setModelPrice,
} from "../api";

function displayRate(rate: number | null): string {
  return rate == null ? "" : String(rate / 1_000_000);
}

function rateMicros(raw: string, label: string): number {
  const value = Number(raw);
  const micros = Math.round(value * 1_000_000);
  if (!/^\d+(?:\.\d{1,6})?$/.test(raw)
      || !Number.isFinite(value)
      || value < 0
      || value > 9_000_000_000
      || !Number.isSafeInteger(micros)) {
    throw new Error(`${label}必须是 0–90 亿之间、最多 6 位小数的有效金额`);
  }
  return micros;
}

export default function PricingEditor() {
  const [table, setTable] = useState<PriceTableView | null>(null);
  const [model, setModel] = useState("");
  const [input, setInput] = useState("0");
  const [output, setOutput] = useState("0");
  const [cacheRead, setCacheRead] = useState("0");
  const [cacheWrite, setCacheWrite] = useState("0");
  const [reasoning, setReasoning] = useState("");
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");

  useEffect(() => {
    getPriceTable().then(setTable).catch((value) => setError(String(value)));
  }, []);

  const edit = (name: string, price: ModelPriceView) => {
    setModel(name);
    setInput(displayRate(price.input_per_mtok));
    setOutput(displayRate(price.output_per_mtok));
    setCacheRead(displayRate(price.cache_read_per_mtok));
    setCacheWrite(displayRate(price.cache_write_per_mtok));
    setReasoning(displayRate(price.reasoning_per_mtok));
    setError("");
    setNotice("");
  };

  const save = async () => {
    setError("");
    setNotice("");
    if (!table) return;
    if (!model || model.trim() !== model || model.length > 256) {
      setError("模型 ID 必须是 1–256 个字符，且首尾不能有空格");
      return;
    }
    try {
      const price: ModelPriceView = {
        input_per_mtok: rateMicros(input, "输入价格"),
        output_per_mtok: rateMicros(output, "输出价格"),
        cache_read_per_mtok: rateMicros(cacheRead, "缓存读取价格"),
        cache_write_per_mtok: rateMicros(cacheWrite, "缓存写入价格"),
        reasoning_per_mtok: reasoning === "" ? null : rateMicros(reasoning, "推理价格"),
      };
      const next = await setModelPrice(model, price, table.version);
      setTable(next);
      setNotice(`已生成 price v${next.version}；正在运行的代理需重新应用配置`);
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
      }
      setNotice(`已生成 price v${next.version}；历史回执保持原成本`);
    } catch (value) {
      setError(String(value));
    }
  };

  return (
    <div className="pricing-section">
      <div className="budget-title-row">
        <div>
          <h3>版本化模型定价</h3>
          <p>金额单位：账户货币 / 1M tokens。每次实际改价生成新版本，历史回执不会重算。</p>
        </div>
        <span className="price-version">price v{table?.version ?? "—"}</span>
      </div>

      {table && Object.keys(table.models).length === 0 && (
        <div className="empty sm">尚未配置模型价格；缺失模型的成本保持未知。</div>
      )}
      {table && Object.keys(table.models).length > 0 && (
        <table className="grid-table price-table">
          <thead>
            <tr>
              <th>模型</th><th>输入</th><th>输出</th><th>缓存读</th><th>缓存写</th><th>推理</th><th>操作</th>
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
                <td>{price.reasoning_per_mtok == null ? "跟随输出" : displayRate(price.reasoning_per_mtok)}</td>
                <td className="price-actions">
                  <button className="btn tiny" aria-label={`编辑 ${name}`} onClick={() => edit(name, price)}>编辑</button>
                  <button className="btn tiny danger" aria-label={`删除 ${name}`} onClick={() => remove(name)}>删除</button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}

      <div className="price-form">
        <label className="field-label price-model-field">
          模型 ID
          <input aria-label="模型 ID" className="input" value={model} onChange={(event) => setModel(event.target.value)} />
        </label>
        {[
          ["输入价格", input, setInput],
          ["输出价格", output, setOutput],
          ["缓存读取价格", cacheRead, setCacheRead],
          ["缓存写入价格", cacheWrite, setCacheWrite],
          ["推理价格", reasoning, setReasoning],
        ].map(([label, value, setter]) => (
          <label className="field-label" key={label as string}>
            {label as string}{label === "推理价格" ? "（空=跟随输出）" : ""}
            <input
              aria-label={label as string}
              className="input"
              type="number"
              min="0"
              step="0.000001"
              value={value as string}
              onChange={(event) => (setter as (value: string) => void)(event.target.value)}
            />
          </label>
        ))}
        <div className="price-save-action">
          <button className="btn primary" disabled={!table} onClick={save}>保存新版本</button>
        </div>
      </div>
      {error && <div className="banner err">{error}</div>}
      {notice && <div className="banner ok">{notice}</div>}
    </div>
  );
}
