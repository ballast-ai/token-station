import { useEffect, useMemo, useRef, useState } from "react";
import {
  checkDesktopUpdate,
  exportRecoveryBundle,
  getRecoveryDiagnostics,
  getRecoveryState,
  openRecoveryFolder,
  recordFrontendDiagnostic,
  installDesktopUpdateAndRestart,
  type DiagnosticPreview,
  type DesktopUpdateView,
  type RecoveryState,
} from "../api";
import { diagnosticInput } from "../diagnostics";
import { humanizeAppError } from "../errors";
import { useLocalizedCopy } from "./LanguageProvider";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "./ui/alert-dialog";

interface RecoveryShellProps {
  initialState?: RecoveryState;
  initialError?: Error;
}

function failureText(error: unknown): string {
  return humanizeAppError(error);
}

function requiresFreshUpdateCheck(message: string): boolean {
  return message.includes("update_version_changed:")
    || message.includes("update_expected_version_missing:");
}

function invalidateUpdateCandidate(
  update: DesktopUpdateView | null,
  message: string,
): DesktopUpdateView | null {
  return update ? {
    ...update,
    status: "unavailable",
    version: null,
    notes: null,
    pub_date: null,
    message,
  } : null;
}

export default function RecoveryShell({ initialState, initialError }: RecoveryShellProps) {
  const { copy } = useLocalizedCopy();
  const [state, setState] = useState<RecoveryState | null>(initialState ?? null);
  const [preview, setPreview] = useState<DiagnosticPreview | null>(null);
  const [confirmed, setConfirmed] = useState(false);
  const [busy, setBusy] = useState("");
  const [error, setError] = useState("");
  const [message, setMessage] = useState("");
  const [upgrade, setUpgrade] = useState<DesktopUpdateView | null>(null);
  const [confirmUpdate, setConfirmUpdate] = useState(false);
  const cancelUpdateRef = useRef<HTMLButtonElement>(null);

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
      return copy(
        "Copied the redacted diagnostic preview",
        "已复制经过二次脱敏的诊断预览",
      );
    });
  };

  const check = async () => {
    setBusy("upgrade");
    setError("");
    try {
      setUpgrade(await checkDesktopUpdate());
    } catch (caught) {
      setError(failureText(caught));
    } finally {
      setBusy("");
    }
  };

  const installUpdate = async () => {
    const expectedVersion = upgrade?.version;
    if (!expectedVersion) {
      const message = copy(
        "The selected update version is missing; check again.",
        "缺少已确认的更新版本，请重新检查。",
      );
      setConfirmUpdate(false);
      setUpgrade((current) => invalidateUpdateCandidate(current, message));
      setError(message);
      return;
    }
    setConfirmUpdate(false);
    setBusy("install-update");
    setError("");
    try {
      const started = await installDesktopUpdateAndRestart(expectedVersion);
      if (!started) {
        setUpgrade(await checkDesktopUpdate());
        setBusy("");
      }
    } catch (caught) {
      const message = failureText(caught);
      if (requiresFreshUpdateCheck(message)) {
        setUpgrade((current) => invalidateUpdateCandidate(current, message));
      } else {
        setError(message);
      }
      setBusy("");
    }
  };

  return (
    <main className="recovery-shell">
      <section className="recovery-card" aria-live="polite">
        <div className="recovery-mark" aria-hidden="true">TS</div>
        <div>
          <p className="eyebrow">{copy(
            "READ-ONLY · LOCAL · NO AUTOMATIC UPLOAD",
            "只读 · 本地 · 不自动上传",
          )}</p>
          <h1>{copy("Token Station recovery mode", "Token Station 自救模式")}</h1>
          <p className="sub">
            {copy(
              "This screen does not depend on the metrics database. You can check for updates, create a read-only export, open the local backup folder, copy diagnostics, or retry.",
              "该界面不依赖业务指标库。这里只提供检查更新、只读导出、打开本地备份位置、复制诊断和重试。",
            )}
          </p>
        </div>

        {(initialError || state?.message) && (
          <div className="banner warn recovery-reason">
            <b>{initialError
              ? copy("Interface rendering error", "前端渲染异常")
              : copy("Database compatibility protection", "数据库兼容性保护")}</b>
            <span>{humanizeAppError(initialError ?? state?.message)}</span>
          </div>
        )}
        {state?.found_schema != null && (
          <div className="kv-grid">
            <div className="kv-k">{copy("Detected schema", "检测到 schema")}</div>
            <div className="kv-v mono">v{state.found_schema}</div>
            <div className="kv-k">{copy("Supported schema", "当前支持")}</div>
            <div className="kv-v mono">v{state.supported_schema ?? "—"}</div>
            <div className="kv-k">{copy("Metrics database", "指标库")}</div>
            <div className="kv-v mono">{state.metrics_path}</div>
          </div>
        )}

        {error && <div className="banner err">{error}</div>}
        {message && <div className="banner ok mono">{message}</div>}
        {upgrade && (
          <div className={`banner ${upgrade.status === "unavailable" ? "err" : upgrade.status === "update_available" ? "warn" : "ok"}`}>
            {upgrade.status === "update_available"
              ? copy(`Version ${upgrade.version} is available`, `发现新版本 ${upgrade.version}`)
              : upgrade.status === "up_to_date"
                ? copy(`Version ${upgrade.current_version} is up to date`, `当前已是最新版本 ${upgrade.current_version}`)
                : humanizeAppError(upgrade.message)}
            {upgrade.release_url && <span className="mono"> · {upgrade.release_url}</span>}
          </div>
        )}

        <div className="recovery-actions">
          <button
            className="btn"
            disabled={Boolean(busy)}
            onClick={upgrade?.status === "update_available" ? () => setConfirmUpdate(true) : () => void check()}
          >
            {busy === "upgrade"
              ? copy("Checking…", "检查中…")
              : busy === "install-update"
                ? copy("Verifying and installing…", "正在验证并安装…")
                : upgrade?.status === "update_available"
                  ? copy(`Download and update to ${upgrade.version}`, `下载并更新到 ${upgrade.version}`)
                  : copy("Check for updates", "检查更新")}
          </button>
          <button
            className="btn"
            disabled={Boolean(busy)}
            onClick={() => void run("open", async () => {
              const path = await openRecoveryFolder();
              return copy(`Opened: ${path}`, `已打开：${path}`);
            })}
          >
            {copy("Open backup folder", "打开备份位置")}
          </button>
          <button className="btn" disabled={Boolean(busy)} onClick={() => void copyDiagnostics()}>
            {copy("Copy redacted diagnostics", "复制脱敏诊断")}
          </button>
          <button className="btn" onClick={() => window.location.reload()}>
            {copy("Run checks again", "重新检测")}
          </button>
        </div>

        <AlertDialog open={confirmUpdate} onOpenChange={setConfirmUpdate}>
          <AlertDialogContent
            onOpenAutoFocus={(event) => {
              event.preventDefault();
              cancelUpdateRef.current?.focus();
            }}
          >
            <AlertDialogHeader>
              <AlertDialogTitle>{copy("Install app update?", "安装应用更新？")}</AlertDialogTitle>
              <AlertDialogDescription>{copy(
                "The signed update will be downloaded and verified first. The local gateway stops only for final installation, then the app restarts.",
                "将先下载并验证签名更新；仅在最终安装时安全停止本地网关，随后 App 自动重启。",
              )}</AlertDialogDescription>
            </AlertDialogHeader>
            <AlertDialogFooter>
              <AlertDialogCancel ref={cancelUpdateRef}>{copy("Cancel", "取消")}</AlertDialogCancel>
              <AlertDialogAction onClick={() => void installUpdate()}>
                {copy("Confirm update and restart", "确认更新并重启")}
              </AlertDialogAction>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>

        <div className="recovery-export">
          <h2>{copy("Read-only export", "只读导出")}</h2>
          <p>
            {copy(
              "The bundle is written only to the local recovery folder and is never uploaded. It may contain the original configuration, metrics database, and SQLite sidecars. Interface diagnostics are redacted again.",
              "导出包仅写入本地自救目录，不上传。它可能包含原始配置、原始指标库及 SQLite sidecar；前端诊断会再次脱敏。",
            )}
          </p>
          {preview?.export_includes.length ? (
            <ul>{preview.export_includes.map((item) => <li key={item}>{item}</li>)}</ul>
          ) : null}
          <label className="recovery-confirm">
            <input type="checkbox" checked={confirmed} onChange={(event) => setConfirmed(event.target.checked)} />
            <span>{copy(
              "I understand this exports the listed raw local data, and I will protect the exported file.",
              "确认导出上述原始本地数据，并自行保管导出文件。",
            )}</span>
          </label>
          <button
            className="btn primary"
            disabled={!confirmed || Boolean(busy)}
            onClick={() => void run("export", async () => {
              const path = await exportRecoveryBundle(true);
              return copy(`Exported: ${path}`, `已导出：${path}`);
            })}
          >
            {busy === "export" ? copy("Exporting…", "导出中…") : copy("Export recovery bundle", "导出自救包")}
          </button>
        </div>

        <details className="recovery-diagnostics">
          <summary>{copy(
            "Diagnostic preview (allowlisted fields, size limits, and a second redaction pass)",
            "诊断预览（字段白名单、长度预算、二次脱敏）",
          )}</summary>
          <pre>{diagnosticText}</pre>
        </details>
      </section>
    </main>
  );
}
