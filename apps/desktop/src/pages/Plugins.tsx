import { useEffect, useState } from "react";
import { PluginsView, getPlugins } from "../api";
import { LanguageBoundary, useLanguage } from "../components/LanguageProvider";
import { Button } from "../components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "../components/ui/card";
import { humanizeAppError } from "../errors";

/// Plugins page: discovered plugin directory plus the monospace list shared with CLI `plugin list`.
function PluginsContent() {
  const { t } = useLanguage();
  const [pv, setPv] = useState<PluginsView | null>(null);
  const [err, setErr] = useState("");

  const load = () => {
    setErr("");
    getPlugins()
      .then(setPv)
      .catch((e) => setErr(humanizeAppError(e)));
  };
  useEffect(load, []);

  return (
    <Card className="settings-card">
      <CardHeader className="panel-head">
        <CardTitle><h2>{t("plugins.title")}</h2></CardTitle>
        <p className="sub">{t("plugins.description")}</p>
      </CardHeader>
      <CardContent className="settings-card-content">

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
            <Button variant="outline" onClick={load}>
              {t("plugins.refresh")}
            </Button>
            <span className="foot-hint">{t("plugins.installHint")}</span>
          </div>
        </>
      )}
      </CardContent>
    </Card>
  );
}

export default function Plugins() {
  return (
    <LanguageBoundary>
      <PluginsContent />
    </LanguageBoundary>
  );
}
