import { useEffect, useMemo, useState } from "react";
import {
  checkUpgrade,
  exportRecoveryBundle,
  getRecoveryDiagnostics,
  getRecoveryState,
  openRecoveryFolder,
  recordFrontendDiagnostic,
  type DiagnosticPreview,
  type RecoveryState,
  type UpgradeView,
} from "../api";
import { diagnosticInput } from "../diagnostics";

interface RecoveryShellProps {
  initialState?: RecoveryState;
  initialError?: Error;
}

function failureText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export default function RecoveryShell({ initialState, initialError }: RecoveryShellProps) {
  const [state, setState] = useState<RecoveryState | null>(initialState ?? null);
  const [preview, setPreview] = useState<DiagnosticPreview | null>(null);
  const [confirmed, setConfirmed] = useState(false);
  const [busy, setBusy] = useState("");
  const [error, setError] = useState("");
  const [message, setMessage] = useState("");
  const [upgrade, setUpgrade] = useState<UpgradeView | null>(null);

  useEffect(() => {
    let disposed = false;
    if (initialError) {
      void recordFrontendDiagnostic(
        diagnosticInput("render_error", initialError),
      ).catch(() => undefined);
    }
    const load = async () => {
      try {
        const nextState = initialState ?? await getRecoveryState();
        const nextPreview = await getRecoveryDiagnostics();
        if (!disposed) {
          setState(nextState);
          setPreview(nextPreview);
        }
      } catch (caught) {
        if (!disposed) setError(failureText(caught));
      }
    };
    void load();
    return () => { disposed = true; };
  }, [initialError, initialState]);

  const diagnosticText = useMemo(() => JSON.stringify({
    recovery: preview?.recovery ?? null,
    frontend_events: preview?.frontend_events ?? [],
    local_only: true,
    redacted: true,
    auto_upload: false,
  }, null, 2), [preview]);

  const run = async (name: string, action: () => Promise<string>) => {
    setBusy(name);
    setError("");
    setMessage("");
    try {
      setMessage(await action());
    } catch (caught) {
      setError(failureText(caught));
    } finally {
      setBusy("");
    }
  };

  const copyDiagnostics = async () => {
    await run("copy", async () => {
      await navigator.clipboard.writeText(diagnosticText);
      return "已复制经过二次脱敏的诊断预览";
    });
  };

  const check = async () => {
    setBusy("upgrade");
    setError("");
    try {
      setUpgrade(await checkUpgrade());
    } catch (caught) {
      setError(failureText(caught));
    } finally {
      setBusy("");
    }
  };

  return (
    <main className="recovery-shell">
      <section className="recovery-card" aria-live="polite">
        <div className="recovery-mark" aria-hidden="true">TS</div>
        <div>
          <p className="eyebrow">只读 · 本地 · 不自动上传</p>
          <h1>Token Station 自救模式</h1>
          <p className="sub">
            该界面不依赖业务指标库。这里只提供检查更新、只读导出、打开本地备份位置、复制诊断和重试。
          </p>
        </div>

        {(initialError || state?.message) && (
          <div className="banner warn recovery-reason">
            <b>{initialError ? "前端渲染异常" : "数据库兼容性保护"}</b>
            <span>{initialError?.message ?? state?.message}</span>
          </div>
        )}
        {state?.found_schema != null && (
          <div className="kv-grid">
            <div className="kv-k">检测到 schema</div><div className="kv-v mono">v{state.found_schema}</div>
            <div className="kv-k">当前支持</div><div className="kv-v mono">v{state.supported_schema ?? "—"}</div>
            <div className="kv-k">指标库</div><div className="kv-v mono">{state.metrics_path}</div>
          </div>
        )}

        {error && <div className="banner err">{error}</div>}
        {message && <div className="banner ok mono">{message}</div>}
        {upgrade && (
          <div className={`banner ${upgrade.newer ? "warn" : "ok"}`}>
            {upgrade.newer ? `发现新版本 ${upgrade.latest_tag}` : `当前已是最新版本 ${upgrade.current}`}
            {upgrade.html_url && <span className="mono"> · {upgrade.html_url}</span>}
          </div>
        )}

        <div className="recovery-actions">
          <button className="btn" disabled={Boolean(busy)} onClick={() => void check()}>
            {busy === "upgrade" ? "检查中…" : "检查更新"}
          </button>
          <button className="btn" disabled={Boolean(busy)} onClick={() => void run("open", async () => `已打开：${await openRecoveryFolder()}`)}>
            打开备份位置
          </button>
          <button className="btn" disabled={Boolean(busy)} onClick={() => void copyDiagnostics()}>
            复制脱敏诊断
          </button>
          <button className="btn" onClick={() => window.location.reload()}>重新检测</button>
        </div>

        <div className="recovery-export">
          <h2>只读导出</h2>
          <p>
            导出包仅写入本地自救目录，不上传。它可能包含原始配置、原始指标库及 SQLite sidecar；前端诊断会再次脱敏。
          </p>
          {preview?.export_includes.length ? (
            <ul>{preview.export_includes.map((item) => <li key={item}>{item}</li>)}</ul>
          ) : null}
          <label className="recovery-confirm">
            <input type="checkbox" checked={confirmed} onChange={(event) => setConfirmed(event.target.checked)} />
            <span>确认导出上述原始本地数据，并自行保管导出文件</span>
          </label>
          <button
            className="btn primary"
            disabled={!confirmed || Boolean(busy)}
            onClick={() => void run("export", async () => `已导出：${await exportRecoveryBundle(true)}`)}
          >
            {busy === "export" ? "导出中…" : "导出自救包"}
          </button>
        </div>

        <details className="recovery-diagnostics">
          <summary>诊断预览（字段白名单、长度预算、二次脱敏）</summary>
          <pre>{diagnosticText}</pre>
        </details>
      </section>
    </main>
  );
}
