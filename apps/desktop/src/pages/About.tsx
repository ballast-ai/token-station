import { useState } from "react";
import { UpgradeView, checkUpgrade } from "../api";

/// About/Updates page. Perform only an anonymous version check, the core’s only permitted outbound connection, and do not replace the binary.
export default function About({ version }: { version: string }) {
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
        <h2>关于 · 更新</h2>
        <p className="sub">
          匿名检查最新发布(唯一非上游的外联,serve 永不主动联网)。只比对版本,不自动下载替换。
        </p>
      </div>

      <div className="kv-grid">
        <div className="kv-k">当前版本</div>
        <div className="kv-v mono">{version}</div>
      </div>

      {err && <div className="banner err">{err}</div>}

      {uv && (
        <div className={`banner ${uv.newer ? "warn" : "ok"}`}>
          {uv.newer ? (
            <>
              有新版本 <b>{uv.latest_tag}</b>(当前 {uv.current})。
            </>
          ) : (
            <>已是最新({uv.current},最新发布 {uv.latest_tag})。</>
          )}
          {uv.html_url && (
            <span className="inline-url">
              <span className="mono">{uv.html_url}</span>
              <button className="btn tiny" onClick={() => copy(uv.html_url)}>
                {copied ? "已复制" : "复制链接"}
              </button>
            </span>
          )}
        </div>
      )}

      <div className="panel-foot">
        <button className="btn primary" disabled={busy} onClick={check}>
          {busy ? "检查中…" : "检查更新"}
        </button>
      </div>
    </section>
  );
}
