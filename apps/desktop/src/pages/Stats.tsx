import { useEffect, useState } from "react";
import { StatsView, getStats } from "../api";

const SINCE = [
  { v: "all", label: "全部" },
  { v: "24h", label: "近 24h" },
  { v: "7d", label: "近 7 天" },
];
const BY = [
  { v: "", label: "总计" },
  { v: "upstream", label: "按供应商" },
  { v: "model", label: "按模型" },
  { v: "pool", label: "按档位" },
  { v: "status", label: "按状态码" },
];

function cost(micros: number | null): string {
  return micros == null ? "—" : `${(micros / 1_000_000).toFixed(4)}`;
}

export default function Stats() {
  const [since, setSince] = useState("all");
  const [by, setBy] = useState("");
  const [data, setData] = useState<StatsView | null>(null);
  const [err, setErr] = useState("");

  useEffect(() => {
    setErr("");
    getStats(since, by || null)
      .then(setData)
      .catch((e) => setErr(String(e)));
  }, [since, by]);

  return (
    <section className="panel">
      <div className="panel-head">
        <h2>用量</h2>
        <p className="sub">只读聚合本地指标库。请求元数据(延迟/token/落档),永不含 prompt 内容。</p>
      </div>

      <div className="add-row">
        <select className="select" value={since} onChange={(e) => setSince(e.target.value)}>
          {SINCE.map((s) => (
            <option key={s.v} value={s.v}>
              {s.label}
            </option>
          ))}
        </select>
        <select className="select" value={by} onChange={(e) => setBy(e.target.value)}>
          {BY.map((b) => (
            <option key={b.v} value={b.v}>
              {b.label}
            </option>
          ))}
        </select>
      </div>

      {err && <div className="banner err">{err}</div>}

      {data?.empty && (
        <div className="empty">
          指标库还没建。开启「设置 · 本地指标」并启动一次代理、跑过请求后,这里就有数据了。
        </div>
      )}

      {data && !data.empty && (
        <>
          <div className="stat-cards">
            <div className="stat-card">
              <div className="stat-num">{data.total.requests}</div>
              <div className="stat-lbl">请求</div>
            </div>
            <div className="stat-card">
              <div className="stat-num">
                {data.total.errors}
                <span className="stat-sub">
                  {" "}
                  ({data.total.requests ? Math.round((data.total.errors / data.total.requests) * 100) : 0}%)
                </span>
              </div>
              <div className="stat-lbl">错误</div>
            </div>
            <div className="stat-card">
              <div className="stat-num">{data.total.p50_latency_ms}<span className="stat-sub"> / {data.total.p95_latency_ms} ms</span></div>
              <div className="stat-lbl">延迟 p50 / p95</div>
            </div>
            <div className="stat-card">
              <div className="stat-num">
                {data.total.input_tokens}<span className="stat-sub"> ↓ {data.total.output_tokens} ↑</span>
              </div>
              <div className="stat-lbl">token 入 / 出</div>
            </div>
            <div className="stat-card">
              <div className="stat-num">{cost(data.total.cost_micros)}</div>
              <div className="stat-lbl">成本(定价表就绪前为 —)</div>
            </div>
          </div>

          {data.groups.length > 0 && (
            <table className="grid-table">
              <thead>
                <tr>
                  <th>{BY.find((b) => b.v === by)?.label ?? by}</th>
                  <th>请求</th>
                  <th>错误</th>
                  <th>p50</th>
                  <th>p95</th>
                  <th>token 入</th>
                  <th>token 出</th>
                  <th>成本</th>
                </tr>
              </thead>
              <tbody>
                {data.groups.map(([k, a]) => (
                  <tr key={k}>
                    <td className="mono">{k}</td>
                    <td>{a.requests}</td>
                    <td>{a.errors}</td>
                    <td>{a.p50_latency_ms}</td>
                    <td>{a.p95_latency_ms}</td>
                    <td>{a.input_tokens}</td>
                    <td>{a.output_tokens}</td>
                    <td>{cost(a.cost_micros)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </>
      )}
    </section>
  );
}
