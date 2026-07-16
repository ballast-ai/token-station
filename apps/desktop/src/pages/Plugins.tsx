import { useEffect, useState } from "react";
import { PluginsView, getPlugins } from "../api";

/// Plugins page: discovered plugin directory plus the monospace list shared with CLI `plugin list`.
export default function Plugins() {
  const [pv, setPv] = useState<PluginsView | null>(null);
  const [err, setErr] = useState("");

  const load = () => {
    setErr("");
    getPlugins()
      .then(setPv)
      .catch((e) => setErr(String(e)));
  };
  useEffect(load, []);

  return (
    <section className="panel">
      <div className="panel-head">
        <h2>插件</h2>
        <p className="sub">
          provider / agent 适配器都是 WASM 沙箱(无网络、拿不到明文 Key)。安装/开发走 CLI,这里只读展示。
        </p>
      </div>

      {err && <div className="banner err">{err}</div>}

      {pv && (
        <>
          <div className="kv-grid">
            <div className="kv-k">插件目录</div>
            <div className="kv-v mono">{pv.dir}</div>
            <div className="kv-k">入站适配器</div>
            <div className="kv-v mono">{pv.agent}</div>
            <div className="kv-k">支持方言</div>
            <div className="kv-v">
              {pv.dialects.length ? (
                pv.dialects.map((d) => (
                  <span className="chip" key={d}>
                    {d}
                  </span>
                ))
              ) : (
                <span className="muted">无</span>
              )}
            </div>
          </div>

          <div className="layer">
            <div className="layer-head">plugin list</div>
            <pre className="mono block">{pv.listing.trimEnd() || "(空)"}</pre>
          </div>

          <div className="panel-foot">
            <button className="btn" onClick={load}>
              刷新
            </button>
            <span className="foot-hint">安装:CLI `token-station plugin install &lt;路径&gt;`</span>
          </div>
        </>
      )}
    </section>
  );
}
