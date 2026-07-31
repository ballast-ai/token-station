import { useState } from "react";
import { UpgradeView, checkUpgrade } from "../api";
import { LanguageBoundary, useLanguage } from "../components/LanguageProvider";

/// About/Updates page. Perform only an anonymous version check, the core’s only permitted outbound connection, and do not replace the binary.
function AboutContent({
  desktopVersion,
  coreVersion,
}: {
  desktopVersion: string;
  coreVersion: string;
}) {
  const { t } = useLanguage();
  const [uv, setUv] = useState<UpgradeView | null>(null);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");
  const [copied, setCopied] = useState(false);

  const check = async () => {
    setBusy(true);
    setErr("");
    setUv(null);
    try {
      setUv(await checkUpgrade());
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(false);
    }
  };

  const copy = (url: string) => {
    navigator.clipboard.writeText(url);
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };

  return (
    <section className="panel">
      <div className="panel-head">
        <h2>{t("about.title")}</h2>
        <p className="sub">{t("about.description")}</p>
      </div>

      <div className="kv-grid">
        <div className="kv-k">Desktop version</div>
        <div className="kv-v mono">{desktopVersion}</div>
        <div className="kv-k">Core version</div>
        <div className="kv-v mono">{coreVersion}</div>
      </div>

      {err && <div className="banner err">{err}</div>}

      {uv && (
        <div className={`banner ${uv.status === "unavailable" ? "err" : uv.newer ? "warn" : "ok"}`}>
          {uv.status === "no_published_release" || uv.status === "unavailable" ? (
            <>{uv.message}</>
          ) : uv.newer ? (
            <>{t("about.newVersion", { latest: uv.latest_tag, current: uv.current })}</>
          ) : (
            <>{t("about.latest", { current: uv.current, latest: uv.latest_tag })}</>
          )}
          {uv.html_url && (
            <span className="inline-url">
              <span className="mono">{uv.html_url}</span>
              <button className="btn tiny" onClick={() => copy(uv.html_url)}>
                {copied ? t("about.copied") : t("about.copyLink")}
              </button>
            </span>
          )}
        </div>
      )}

      <div className="panel-foot">
        <button className="btn primary" disabled={busy} onClick={check}>
          {busy ? t("about.checking") : t("about.check")}
        </button>
      </div>
    </section>
  );
}

export default function About(props: Parameters<typeof AboutContent>[0]) {
  return (
    <LanguageBoundary>
      <AboutContent {...props} />
    </LanguageBoundary>
  );
}
