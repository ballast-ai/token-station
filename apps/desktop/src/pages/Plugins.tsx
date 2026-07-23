import { useEffect, useState } from "react";
import { PluginsView, getPlugins } from "../api";
import { LanguageBoundary, useLanguage } from "../components/LanguageProvider";

/// Plugins page: discovered plugin directory plus the monospace list shared with CLI `plugin list`.
function PluginsContent() {
  const { t } = useLanguage();
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
        <h2>{t("plugins.title")}</h2>
        <p className="sub">{t("plugins.description")}</p>
      </div>

      {err && <div className="banner err">{err}</div>}

      {pv && (
        <>
          <div className="kv-grid">
            <div className="kv-k">{t("plugins.directory")}</div>
            <div className="kv-v mono">{pv.dir}</div>
            <div className="kv-k">{t("plugins.adapter")}</div>
            <div className="kv-v mono">{pv.agent}</div>
            <div className="kv-k">{t("plugins.dialects")}</div>
            <div className="kv-v">
              {pv.dialects.length ? (
                pv.dialects.map((d) => (
                  <span className="chip" key={d}>
                    {d}
                  </span>
                ))
              ) : (
                <span className="muted">{t("plugins.none")}</span>
              )}
            </div>
          </div>

          <div className="layer">
            <div className="layer-head">plugin list</div>
            <pre className="mono block">{pv.listing.trimEnd() || t("plugins.empty")}</pre>
          </div>

          <div className="panel-foot">
            <button className="btn" onClick={load}>
              {t("plugins.refresh")}
            </button>
            <span className="foot-hint">{t("plugins.installHint")}</span>
          </div>
        </>
      )}
    </section>
  );
}

export default function Plugins() {
  return (
    <LanguageBoundary>
      <PluginsContent />
    </LanguageBoundary>
  );
}
