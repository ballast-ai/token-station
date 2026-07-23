import { useEffect, useState } from "react";
import { getEgress, SettingsView, StateView, setSettings, type EgressView } from "../api";
import { LanguageBoundary, useLanguage } from "../components/LanguageProvider";

/// Settings page for proxy switches, egress policy, and read-only environment details.
/// Switch changes do not affect a running server until the proxy restarts; the panel states this explicitly.
function SettingsContent({
  settings,
  serveRunning,
  onSaved,
}: {
  settings: SettingsView;
  serveRunning: boolean;
  onSaved: (s: StateView) => void;
}) {
  const { t } = useLanguage();
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
      setOk(serveRunning ? t("general.savedRestart") : t("general.saved"));
    } catch (e) {
      setErr(String(e));
    }
  };

  return (
    <section className="panel">
      <div className="panel-head">
        <h2>{t("general.title")}</h2>
        <p className="sub">{t("general.description")}</p>
      </div>

      {err && <div className="banner err">{err}</div>}
      {ok && <div className="banner ok">{ok}</div>}

      <div className="setting-row">
        <label className="switch">
          <input type="checkbox" checked={auth} onChange={(e) => setAuth(e.target.checked)} />
          <span>
            <b>{t("general.auth")}</b>(server.auth)
            <em>{t("general.authDescription")}</em>
          </span>
        </label>
      </div>

      <div className="setting-row egress-settings">
        <div>
          <b>{t("general.egress")}</b>
          <em>{t("general.egressDescription")}</em>
        </div>
        <label>
          {t("general.egressMode")}
          <select aria-label={t("general.egressMode")} value={egressMode} onChange={(event) => setEgressMode(event.target.value as typeof egressMode)}>
            <option value="direct">{t("general.direct")}</option>
            <option value="http">HTTP CONNECT</option>
            <option value="socks5">SOCKS5</option>
          </select>
        </label>
        {egressMode !== "direct" && (
          <>
            <label>{t("general.proxyUrl")}<input aria-label={t("general.proxyUrl")} value={proxyUrl} onChange={(event) => setProxyUrl(event.target.value)} placeholder={egressMode === "http" ? "http://proxy.company:8080" : "socks5h://127.0.0.1:1080"} /></label>
            <label>{t("general.noProxy")}<input aria-label={t("general.noProxy")} value={noProxy} onChange={(event) => setNoProxy(event.target.value)} placeholder="localhost, *.corp.internal" /></label>
            <label>{t("general.proxyUsername")}<input aria-label={t("general.proxyUsername")} value={proxyUsername} onChange={(event) => setProxyUsername(event.target.value)} /></label>
            <label>{t("general.authSlot")}<input aria-label={t("general.authSlotLabel")} value={proxySlot} onChange={(event) => setProxySlot(event.target.value)} placeholder="corporate_proxy_password" /></label>
            {proxySlot && <div className="inline-note mono">{t("general.credentialCommand", { slot: proxySlot })}</div>}
          </>
        )}
        <div className="inline-note">
          {t("general.routeSummary", {
            route: egressMode === "direct"
              ? t("general.direct")
              : proxyUrl || t("general.pendingProxy"),
          })}
        </div>
        {egressView && (
          <div className="kv-grid" aria-label="实际出口解析">
            {egressView.routes.map((route) => (
              <div key={`${route.request_class}-${route.upstream}`} className="egress-route-row">
                <span>{route.request_class} · {route.upstream}</span>
                <code>{route.route === "direct" ? t("general.direct") : egressView.proxy_url}</code>
                {route.matched_no_proxy && <small>{t("general.matchedNoProxy")}</small>}
              </div>
            ))}
            <div className="egress-route-row"><span>update_check</span><code>{t("general.direct")}</code></div>
          </div>
        )}
      </div>

      <div className="setting-row">
        <label className="switch">
          <input type="checkbox" checked={metrics} onChange={(e) => setMetrics(e.target.checked)} />
          <span>
            <b>{t("general.metrics")}</b>(data.metrics)
            <em>{t("general.metricsDescription")}</em>
          </span>
        </label>
      </div>

      <div className="panel-foot">
        <button className="btn primary" disabled={!dirty} onClick={save}>
          {t("general.save")}
        </button>
        {serveRunning && dirty && <span className="foot-hint">{t("general.restartHint")}</span>}
      </div>

      <div className="kv-grid">
        <div className="kv-k">{t("general.listen")}</div>
        <div className="kv-v mono">{settings.listen}</div>
        <div className="kv-k">{t("general.dataDir")}</div>
        <div className="kv-v mono">{settings.data_dir || "—"}</div>
        <div className="kv-k">{t("general.pluginsDir")}</div>
        <div className="kv-v mono">{settings.plugins_dir || "—"}</div>
        <div className="kv-k">{t("general.adapter")}</div>
        <div className="kv-v mono">{settings.agent || "—"}</div>
        <div className="kv-k">{t("general.coreVersion")}</div>
        <div className="kv-v mono">{settings.version}</div>
      </div>
    </section>
  );
}

export default function Settings(props: Parameters<typeof SettingsContent>[0]) {
  return (
    <LanguageBoundary>
      <SettingsContent {...props} />
    </LanguageBoundary>
  );
}
