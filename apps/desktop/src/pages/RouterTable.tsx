import { useEffect, useState } from "react";
import { RouterTableView, getRouterTable } from "../api";
import { LanguageBoundary, useLanguage } from "../components/LanguageProvider";
import { Card, CardContent, CardHeader, CardTitle } from "../components/ui/card";
import { humanizeAppError } from "../errors";

/// Visualizes the four routing layers in the core's short-circuit order:
/// 1 rules (hard matches) -> 2 agent hints -> 3 heuristic tiers -> 4 default fallback.
function RouterTableContent() {
  const { t } = useLanguage();
  const [rt, setRt] = useState<RouterTableView | null>(null);
  const [err, setErr] = useState("");

  useEffect(() => {
    getRouterTable()
      .then(setRt)
      .catch((e) => setErr(humanizeAppError(e)));
  }, []);

  if (err) return <Card className="settings-card"><CardContent><div className="banner err">{err}</div></CardContent></Card>;
  if (!rt) return <Card className="settings-card"><CardContent><div className="empty">{t("router.loading")}</div></CardContent></Card>;

  const model = (u: string | null, m: string | null) =>
    u ? `${u} · ${m ?? "?"}` : t("router.unconfigured");

  return (
    <Card className="settings-card">
      <CardHeader className="panel-head">
        <CardTitle><h2>{t("router.title")}</h2></CardTitle>
        <p className="sub">{t("router.description")}</p>
      </CardHeader>
      <CardContent className="settings-card-content">

      {/* Layer 1: rules */}
      <div className="layer">
        <div className="layer-head"><span className="layer-no">1</span> {t("router.rules")}</div>
        {rt.rules.length === 0 ? (
          <div className="empty sm">{t("router.noRules")}</div>
        ) : (
          <pre className="mono block">{JSON.stringify(rt.rules, null, 2)}</pre>
        )}
      </div>

      {/* Layer 2: hints */}
      <div className="layer">
        <div className="layer-head"><span className="layer-no">2</span> {t("router.hints")}</div>
        {rt.hint_routes.length === 0 ? (
          <div className="empty sm">{t("router.noHints")}</div>
        ) : (
          <pre className="mono block">{JSON.stringify(rt.hint_routes, null, 2)}</pre>
        )}
      </div>

      {/* Layer 3: heuristic tiers */}
      <div className="layer">
        <div className="layer-head">
          <span className="layer-no">3</span> {t("router.bands")}
          {rt.threshold != null && <span className="layer-note">{t("router.threshold", { value: rt.threshold })}</span>}
        </div>
        {rt.bands.length === 0 ? (
          <div className="empty sm">{t("router.noBands")}</div>
        ) : (
          <table className="grid-table">
            <thead>
              <tr>
                <th>{t("router.score")}</th>
                <th>{t("router.pool")}</th>
                <th>{t("router.target")}</th>
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
        <div className="layer-head"><span className="layer-no">4</span> {t("router.default")}</div>
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
          <div className="kv-k">{t("router.contextWindow")}</div>
          <div className="kv-v mono">{rt.assumed_context_window || "—"}</div>
        </div>
      </div>
      </CardContent>
    </Card>
  );
}

export default function RouterTable() {
  return (
    <LanguageBoundary>
      <RouterTableContent />
    </LanguageBoundary>
  );
}
