import { useState } from "react";
import { SettingsView, StateView, setSettings } from "../api";

/// Settings page: two writable switches (server.auth / data.metrics) and read-only environment information.
/// Switch changes do not affect a running server until the proxy restarts; the panel states this explicitly.
export default function Settings({
  settings,
  serveRunning,
  onSaved,
}: {
  settings: SettingsView;
  serveRunning: boolean;
  onSaved: (s: StateView) => void;
}) {
  const [auth, setAuth] = useState(settings.auth);
  const [metrics, setMetrics] = useState(settings.metrics);
  const [err, setErr] = useState("");
  const [ok, setOk] = useState("");

  const dirty = auth !== settings.auth || metrics !== settings.metrics;

  const save = async () => {
    setErr("");
    setOk("");
    try {
      const s = await setSettings(auth, metrics);
      onSaved(s);
      setOk(serveRunning ? "已保存 · 重启代理后生效" : "已保存");
    } catch (e) {
      setErr(String(e));
    }
  };

  return (
    <section className="panel">
      <div className="panel-head">
        <h2>设置</h2>
        <p className="sub">虚拟 Key 鉴权与本地指标两个开关。其余环境信息只读。</p>
      </div>

      {err && <div className="banner err">{err}</div>}
      {ok && <div className="banner ok">{ok}</div>}

      <div className="setting-row">
        <label className="switch">
          <input type="checkbox" checked={auth} onChange={(e) => setAuth(e.target.checked)} />
          <span>
            <b>虚拟 Key 鉴权</b>(server.auth)
            <em>开启后 serve 首启生成虚拟 Key,所有请求须带 Key。关掉则回环端口裸奔,仅建议本机可信环境。</em>
          </span>
        </label>
      </div>

      <div className="setting-row">
        <label className="switch">
          <input type="checkbox" checked={metrics} onChange={(e) => setMetrics(e.target.checked)} />
          <span>
            <b>本地指标</b>(data.metrics)
            <em>把每次请求的元数据(延迟/token/落档,永不含 prompt 内容)写入本地 SQLite,供「用量」页聚合。</em>
          </span>
        </label>
      </div>

      <div className="panel-foot">
        <button className="btn primary" disabled={!dirty} onClick={save}>
          保存
        </button>
        {serveRunning && dirty && <span className="foot-hint">代理运行中——保存后需重启代理才生效</span>}
      </div>

      <div className="kv-grid">
        <div className="kv-k">监听地址</div>
        <div className="kv-v mono">{settings.listen}</div>
        <div className="kv-k">数据目录</div>
        <div className="kv-v mono">{settings.data_dir || "—"}</div>
        <div className="kv-k">插件目录</div>
        <div className="kv-v mono">{settings.plugins_dir || "—"}</div>
        <div className="kv-k">入站适配器</div>
        <div className="kv-v mono">{settings.agent || "—"}</div>
        <div className="kv-k">内核版本</div>
        <div className="kv-v mono">{settings.version}</div>
      </div>
    </section>
  );
}
