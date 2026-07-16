import { useEffect, useState } from "react";
import { RouterTableView, getRouterTable } from "../api";

/// Visualizes the four routing layers in the core's short-circuit order:
/// 1 rules (hard matches) -> 2 agent hints -> 3 heuristic tiers -> 4 default fallback.
export default function RouterTable() {
  const [rt, setRt] = useState<RouterTableView | null>(null);
  const [err, setErr] = useState("");

  useEffect(() => {
    getRouterTable()
      .then(setRt)
      .catch((e) => setErr(String(e)));
  }, []);

  if (err) return <section className="panel"><div className="banner err">{err}</div></section>;
  if (!rt) return <section className="panel"><div className="empty">加载中…</div></section>;

  const model = (u: string | null, m: string | null) =>
    u ? `${u} · ${m ?? "?"}` : "— 未配 —";

  return (
    <section className="panel">
      <div className="panel-head">
        <h2>路由表 · 四层</h2>
        <p className="sub">
          每个请求按此顺序短路命中:命中即停。你在「主页」配的三档,落在第 3 层的分档里。
        </p>
      </div>

      {/* Layer 1: rules */}
      <div className="layer">
        <div className="layer-head"><span className="layer-no">1</span> 规则(硬匹配)</div>
        {rt.rules.length === 0 ? (
          <div className="empty sm">无规则。规则用于「某条件强制走某池」,当前留空。</div>
        ) : (
          <pre className="mono block">{JSON.stringify(rt.rules, null, 2)}</pre>
        )}
      </div>

      {/* Layer 2: hints */}
      <div className="layer">
        <div className="layer-head"><span className="layer-no">2</span> 提示路由(agent Hint)</div>
        {rt.hint_routes.length === 0 ? (
          <div className="empty sm">无提示路由。入站适配器给出 step_type 等 Hint 时在此分流,当前留空。</div>
        ) : (
          <pre className="mono block">{JSON.stringify(rt.hint_routes, null, 2)}</pre>
        )}
      </div>

      {/* Layer 3: heuristic tiers */}
      <div className="layer">
        <div className="layer-head">
          <span className="layer-no">3</span> 启发式分档
          {rt.threshold != null && <span className="layer-note">阈值 {rt.threshold}</span>}
        </div>
        {rt.bands.length === 0 ? (
          <div className="empty sm">还没配档。去「主页」给三档各选供应商 + 模型。</div>
        ) : (
          <table className="grid-table">
            <thead>
              <tr>
                <th>分数 ≥</th>
                <th>档池</th>
                <th>落到</th>
              </tr>
            </thead>
            <tbody>
              {rt.bands.map((b) => (
                <tr key={b.pool}>
                  <td className="mono">{b.at_least}</td>
                  <td className="mono">{b.pool}</td>
                  <td>{model(b.upstream, b.model)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>

      {/* Layer 4: default fallback */}
      <div className="layer">
        <div className="layer-head"><span className="layer-no">4</span> 默认兜底</div>
        <div className="kv-grid">
          <div className="kv-k">default_pool</div>
          <div className="kv-v">
            <span className="mono">{rt.default_pool || "—"}</span>
            {rt.pools.find((p) => p.pool === rt.default_pool) && (
              <span className="muted">
                {" "}
                → {model(
                  rt.pools.find((p) => p.pool === rt.default_pool)!.upstream,
                  rt.pools.find((p) => p.pool === rt.default_pool)!.model,
                )}
              </span>
            )}
          </div>
          <div className="kv-k">假定上下文窗</div>
          <div className="kv-v mono">{rt.assumed_context_window || "—"}</div>
        </div>
      </div>
    </section>
  );
}
