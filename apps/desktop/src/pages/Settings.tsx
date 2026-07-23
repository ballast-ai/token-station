import { useEffect, useState } from "react";
import { getEgress, SettingsView, StateView, setSettings, type EgressView } from "../api";

/// Settings page for proxy switches, egress policy, and read-only environment details.
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
  const [egressMode, setEgressMode] = useState(settings.egress_mode);
  const [proxyUrl, setProxyUrl] = useState(settings.egress_proxy_url);
  const [noProxy, setNoProxy] = useState(settings.egress_no_proxy.join(", "));
  const [proxyUsername, setProxyUsername] = useState(settings.egress_auth_username);
  const [proxySlot, setProxySlot] = useState(settings.egress_auth_slot);
  const [err, setErr] = useState("");
  const [ok, setOk] = useState("");
  const [egressView, setEgressView] = useState<EgressView | null>(null);

  useEffect(() => {
    void getEgress().then(setEgressView).catch(() => setEgressView(null));
  }, [settings]);

  const noProxyEntries = noProxy.split(",").map((value) => value.trim()).filter(Boolean);
  const dirty = auth !== settings.auth
    || metrics !== settings.metrics
    || egressMode !== settings.egress_mode
    || proxyUrl !== settings.egress_proxy_url
    || noProxyEntries.join(",") !== settings.egress_no_proxy.join(",")
    || proxyUsername !== settings.egress_auth_username
    || proxySlot !== settings.egress_auth_slot;

  const save = async () => {
    setErr("");
    setOk("");
    try {
      const s = await setSettings(auth, metrics, {
        egress_mode: egressMode,
        egress_proxy_url: proxyUrl.trim(),
        egress_no_proxy: noProxyEntries,
        egress_auth_username: proxyUsername.trim(),
        egress_auth_slot: proxySlot.trim(),
      });
      onSaved(s);
      setOk(serveRunning ? "已保存 · 重启代理后生效" : "已保存");
    } catch (e) {
      setErr(String(e));
    }
  };

  return (
    <section className="panel">
      <div className="panel-head">
        <h2>代理与数据</h2>
        <p className="sub">配置虚拟 Key、本地指标和显式出站策略；运行环境信息只读。</p>
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

      <div className="setting-row egress-settings">
        <div>
          <b>出站策略</b>
          <em>显式选择直连、HTTP CONNECT 或 SOCKS5；不会读取系统 HTTP_PROXY / ALL_PROXY。</em>
        </div>
        <label>
          出口模式
          <select aria-label="出口模式" value={egressMode} onChange={(event) => setEgressMode(event.target.value as typeof egressMode)}>
            <option value="direct">直连</option>
            <option value="http">HTTP CONNECT</option>
            <option value="socks5">SOCKS5</option>
          </select>
        </label>
        {egressMode !== "direct" && (
          <>
            <label>代理 URL<input aria-label="代理 URL" value={proxyUrl} onChange={(event) => setProxyUrl(event.target.value)} placeholder={egressMode === "http" ? "http://proxy.company:8080" : "socks5h://127.0.0.1:1080"} /></label>
            <label>no_proxy<input aria-label="no_proxy" value={noProxy} onChange={(event) => setNoProxy(event.target.value)} placeholder="localhost, *.corp.internal" /></label>
            <label>代理用户名<input aria-label="代理用户名" value={proxyUsername} onChange={(event) => setProxyUsername(event.target.value)} /></label>
            <label>认证槽<input aria-label="代理认证槽" value={proxySlot} onChange={(event) => setProxySlot(event.target.value)} placeholder="corporate_proxy_password" /></label>
            {proxySlot && <div className="inline-note mono">写入凭据：token-station-cli key set egress-proxy {proxySlot}</div>}
          </>
        )}
        <div className="inline-note">
          Provider 请求、模型目录与健康探测：{egressMode === "direct" ? "直连" : `经 ${proxyUrl || "待填写代理"}`}；更新检查固定直连。每次 3xx 仍拒绝跟随，跨主机不会转发原 Authorization。
        </div>
        {egressView && (
          <div className="kv-grid" aria-label="实际出口解析">
            {egressView.routes.map((route) => (
              <div key={`${route.request_class}-${route.upstream}`} className="egress-route-row">
                <span>{route.request_class} · {route.upstream}</span>
                <code>{route.route === "direct" ? "直连" : egressView.proxy_url}</code>
                {route.matched_no_proxy && <small>命中 no_proxy</small>}
              </div>
            ))}
            <div className="egress-route-row"><span>update_check</span><code>直连</code></div>
          </div>
        )}
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
