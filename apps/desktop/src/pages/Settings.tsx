import { useEffect, useRef, useState } from "react";
import { getEgress, SettingsView, StateView, setSettings, type EgressView } from "../api";
import { LanguageBoundary, useLanguage } from "../components/LanguageProvider";
import { humanizeAppError } from "../errors";
import { Button } from "../components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "../components/ui/card";
import { Input } from "../components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "../components/ui/select";
import { Switch } from "../components/ui/switch";
import { useErrorToast } from "../components/ErrorToast";

function settingsFailure(caught: unknown): { field: string; message: string } {
  if (caught && typeof caught === "object") {
    const value = caught as { field?: unknown; message?: unknown };
    if (typeof value.field === "string" && typeof value.message === "string") {
      return { field: value.field, message: humanizeAppError(value) };
    }
  }
  return {
    field: "",
    message: humanizeAppError(caught),
  };
}

/// Settings page for proxy switches and egress policy.
/// Switch changes do not affect a running server until the proxy restarts; the panel states this explicitly.
function SettingsContent({
  settings,
  serveRunning,
  onSaved,
  mode = "all",
}: {
  settings: SettingsView;
  serveRunning: boolean;
  onSaved: (s: StateView) => void;
  mode?: "all" | "general" | "api-key";
}) {
  const { t } = useLanguage();
  const { showError, showSuccess } = useErrorToast();
  const [auth, setAuth] = useState(settings.auth);
  const [metrics, setMetrics] = useState(settings.metrics);
  const [egressMode, setEgressMode] = useState(settings.egress_mode);
  const [proxyUrl, setProxyUrl] = useState(settings.egress_proxy_url);
  const [noProxy, setNoProxy] = useState(settings.egress_no_proxy.join(", "));
  const [proxyUsername, setProxyUsername] = useState(settings.egress_auth_username);
  const [proxySlot, setProxySlot] = useState(settings.egress_auth_slot);
  const [err, setErr] = useState("");
  const [errorField, setErrorField] = useState("");
  const [egressView, setEgressView] = useState<EgressView | null>(null);
  const proxyUrlRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    void getEgress().then(setEgressView).catch(() => setEgressView(null));
  }, [settings]);

  const noProxyEntries = noProxy.split(",").map((value) => value.trim()).filter(Boolean);
  const authDirty = auth !== settings.auth;
  const generalDirty = metrics !== settings.metrics
    || egressMode !== settings.egress_mode
    || proxyUrl !== settings.egress_proxy_url
    || noProxyEntries.join(",") !== settings.egress_no_proxy.join(",")
    || proxyUsername !== settings.egress_auth_username
    || proxySlot !== settings.egress_auth_slot;
  const dirty = mode === "api-key"
    ? authDirty
    : mode === "general"
      ? generalDirty
      : authDirty || generalDirty;

  const save = async () => {
    setErr("");
    setErrorField("");
    try {
      const s = await setSettings(auth, metrics, {
        egress_mode: egressMode,
        egress_proxy_url: proxyUrl.trim(),
        egress_no_proxy: noProxyEntries,
        egress_auth_username: proxyUsername.trim(),
        egress_auth_slot: proxySlot.trim(),
      });
      onSaved(s);
      showSuccess(
        serveRunning ? t("general.savedRestart") : t("general.saved"),
        "settings-save",
      );
    } catch (e) {
      const failure = settingsFailure(e);
      if (failure.field === "egress_proxy_url") {
        setErr(failure.message);
        setErrorField(failure.field);
        requestAnimationFrame(() => proxyUrlRef.current?.focus());
      } else {
        showError(failure.message, "settings-save");
      }
    }
  };

  return (
    <Card className={`settings-card general-settings-card ${mode === "api-key" ? "api-key-auth-settings-card" : ""}`}>
      <CardHeader className="panel-head">
        <CardTitle><h2>{t(mode === "api-key" ? "key.authTitle" : "general.title")}</h2></CardTitle>
        <p className="sub">{t(mode === "api-key" ? "key.authSettingsDescription" : "general.description")}</p>
      </CardHeader>
      <CardContent className="settings-card-content">

      {mode !== "general" && <div className="setting-row setting-toggle-row">
        <span id="settings-auth-label" className="setting-toggle-copy">
            <b>{t("general.auth")}</b>(server.auth)
            <em>{t("general.authDescription")}</em>
        </span>
        <Switch aria-labelledby="settings-auth-label" checked={auth} onCheckedChange={setAuth} />
      </div>}

      {mode !== "api-key" && <div className="setting-row egress-settings">
        <div>
          <b>{t("general.egress")}</b>
          <em>{t("general.egressDescription")}</em>
        </div>
        <label>
          {t("general.egressMode")}
          <Select value={egressMode} onValueChange={(value) => setEgressMode(value as typeof egressMode)}>
            <SelectTrigger aria-label={t("general.egressMode")}>
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="direct">{t("general.direct")}</SelectItem>
              <SelectItem value="http">HTTP CONNECT</SelectItem>
              <SelectItem value="socks5">SOCKS5</SelectItem>
            </SelectContent>
          </Select>
        </label>
        {egressMode !== "direct" && (
          <>
            <label>
              {t("general.proxyUrl")}
              <Input
                ref={proxyUrlRef}
                aria-label={t("general.proxyUrl")}
                aria-invalid={errorField === "egress_proxy_url"}
                aria-describedby={errorField === "egress_proxy_url" ? "egress-proxy-url-error" : undefined}
                value={proxyUrl}
                onChange={(event) => {
                  setProxyUrl(event.target.value);
                  if (errorField === "egress_proxy_url") {
                    setErrorField("");
                    setErr("");
                  }
                }}
                placeholder={egressMode === "http" ? "http://proxy.company:8080" : "socks5h://127.0.0.1:1080"}
              />
              {errorField === "egress_proxy_url" && (
                <small id="egress-proxy-url-error" className="error-text" role="alert">{err}</small>
              )}
            </label>
            <label>{t("general.noProxy")}<Input aria-label={t("general.noProxy")} value={noProxy} onChange={(event) => setNoProxy(event.target.value)} placeholder="localhost, *.corp.internal" /></label>
            <label>{t("general.proxyUsername")}<Input aria-label={t("general.proxyUsername")} value={proxyUsername} onChange={(event) => setProxyUsername(event.target.value)} /></label>
            <label>{t("general.authSlot")}<Input aria-label={t("general.authSlotLabel")} value={proxySlot} onChange={(event) => setProxySlot(event.target.value)} placeholder="corporate_proxy_password" /></label>
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
          <div className="egress-route-list" aria-label={t("general.resolvedRoutes")}>
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
      </div>}

      {mode !== "api-key" && <div className="setting-row setting-toggle-row">
        <span id="settings-metrics-label" className="setting-toggle-copy">
            <b>{t("general.metrics")}</b>(data.metrics)
            <em>{t("general.metricsDescription")}</em>
        </span>
        <Switch aria-labelledby="settings-metrics-label" checked={metrics} onCheckedChange={setMetrics} />
      </div>}

      <div className="panel-foot">
        <Button disabled={!dirty} onClick={save}>
          {t("general.save")}
        </Button>
        {serveRunning && dirty && <span className="foot-hint">{t("general.restartHint")}</span>}
      </div>

      </CardContent>
    </Card>
  );
}

export default function Settings(props: Parameters<typeof SettingsContent>[0]) {
  return (
    <LanguageBoundary>
      <SettingsContent {...props} />
    </LanguageBoundary>
  );
}
