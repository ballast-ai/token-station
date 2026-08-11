import { useEffect, useRef, useState } from "react";
import {
  checkDesktopUpdate,
  installDesktopUpdateAndRestart,
  listenDesktopUpdateProgress,
} from "../api";
import type { DesktopUpdateProgress, DesktopUpdateView } from "../api";
import { LanguageBoundary, useLanguage } from "../components/LanguageProvider";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "../components/ui/alert-dialog";
import { Button } from "../components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "../components/ui/card";
import { humanizeAppError } from "../errors";

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

/// About and updates page. It only performs the anonymous version check allowed by the core and never replaces binaries.
function AboutContent({
  desktopVersion,
  coreVersion,
  onOpenFirstRunGuide,
}: {
  desktopVersion: string;
  coreVersion: string;
  onOpenFirstRunGuide?: () => void;
}) {
  const { copy, language, t } = useLanguage();
  const [update, setUpdate] = useState<DesktopUpdateView | null>(null);
  const [busy, setBusy] = useState<"" | "checking" | "installing" | "restarting">("");
  const [err, setErr] = useState("");
  const [copied, setCopied] = useState(false);
  const [confirmOpen, setConfirmOpen] = useState(false);
  const [progress, setProgress] = useState<DesktopUpdateProgress | null>(null);
  const cancelRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listenDesktopUpdateProgress((next) => {
      if (!disposed) setProgress(next);
    }).then((stop) => {
      if (disposed) stop();
      else unlisten = stop;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  const check = async () => {
    setBusy("checking");
    setErr("");
    setUpdate(null);
    try {
      setUpdate(await checkDesktopUpdate());
    } catch (e) {
      setErr(humanizeAppError(e, language));
    } finally {
      setBusy("");
    }
  };

  const install = async () => {
    const expectedVersion = update?.version;
    if (!expectedVersion) {
      const rawMessage = "update_expected_version_missing: selected update version is absent";
      const message = humanizeAppError(rawMessage, language);
      setConfirmOpen(false);
      setUpdate((current) => invalidateUpdateCandidate(current, rawMessage));
      setErr(message);
      return;
    }
    setConfirmOpen(false);
    setBusy("installing");
    setErr("");
    setProgress(null);
    try {
      const started = await installDesktopUpdateAndRestart(expectedVersion);
      if (started) {
        setBusy("restarting");
      } else {
        setUpdate(await checkDesktopUpdate());
        setBusy("");
      }
    } catch (e) {
      const rawMessage = String(e);
      const message = humanizeAppError(rawMessage, language);
      if (requiresFreshUpdateCheck(rawMessage)) {
        setUpdate((current) => invalidateUpdateCandidate(current, rawMessage));
      } else {
        setErr(message);
      }
      setBusy("");
    }
  };

  const copyReleaseUrl = (url: string) => {
    navigator.clipboard.writeText(url);
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };

  return (
    <Card className="settings-card">
      <CardHeader className="panel-head">
        <CardTitle><h2>{t("about.title")}</h2></CardTitle>
        <p className="sub">{t("about.description")}</p>
      </CardHeader>
      <CardContent className="settings-card-content">

      <div className="kv-grid">
        <div className="kv-k">Desktop version</div>
        <div className="kv-v mono">{desktopVersion}</div>
        <div className="kv-k">Core version</div>
        <div className="kv-v mono">{coreVersion}</div>
      </div>

      {err && <div className="banner err">{err}</div>}

      {update && (
        <div
          className={`banner ${update.status === "unavailable" ? "err" : update.status === "update_available" ? "warn" : "ok"}`}
          aria-live="polite"
        >
          {update.status === "update_available" ? (
            <>
              <b>{t("about.updateFound", { version: update.version ?? "" })}</b>
              {update.pub_date && <span>{t("about.releaseDate", { date: update.pub_date })}</span>}
              {update.notes && <span>{update.notes}</span>}
            </>
          ) : update.status === "up_to_date" ? (
            <>{t("about.latest", { current: update.current_version, latest: update.current_version })}</>
          ) : (
            <>{humanizeAppError(update.message, language)}</>
          )}
          {update.release_url && (
            <span className="inline-url">
              <span className="mono">{update.release_url}</span>
              <Button variant="outline" size="sm" onClick={() => copyReleaseUrl(update.release_url)}>
                {copied ? t("about.copied") : t("about.copyLink")}
              </Button>
            </span>
          )}
        </div>
      )}

      {busy === "installing" && progress && (
        <div className="banner" aria-live="polite">
          {progress.total && progress.total > 0 ? (
            <>
              <span>{t("about.downloaded", {
                percent: Math.min(100, Math.floor((progress.downloaded / progress.total) * 100)),
              })}</span>
              <progress
                max={100}
                value={Math.min(100, Math.floor((progress.downloaded / progress.total) * 100))}
                aria-label={t("about.downloadProgress")}
                aria-valuemin={0}
                aria-valuemax={100}
                aria-valuenow={Math.min(100, Math.floor((progress.downloaded / progress.total) * 100))}
              />
            </>
          ) : (
            <span>{t("about.downloadedBytes", { downloaded: progress.downloaded })}</span>
          )}
        </div>
      )}

      <div className="panel-foot">
        <Button
          disabled={Boolean(busy)}
          onClick={update?.status === "update_available" ? () => setConfirmOpen(true) : check}
        >
          {busy === "checking"
            ? t("about.checking")
            : busy === "installing"
              ? t("about.installing")
              : busy === "restarting"
                ? t("about.restarting")
                : update?.status === "update_available"
                  ? t("about.installVersion", { version: update.version ?? "" })
                  : t("about.check")}
        </Button>
        {onOpenFirstRunGuide && (
          <Button variant="outline" type="button" onClick={onOpenFirstRunGuide}>
            {copy("Review getting started guide", "重新查看新手引导")}
          </Button>
        )}
      </div>

      <AlertDialog open={confirmOpen} onOpenChange={setConfirmOpen}>
        <AlertDialogContent
          onOpenAutoFocus={(event) => {
            event.preventDefault();
            cancelRef.current?.focus();
          }}
        >
          <AlertDialogHeader>
            <AlertDialogTitle>{t("about.confirmTitle")}</AlertDialogTitle>
            <AlertDialogDescription>{t("about.confirmDescription")}</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel ref={cancelRef}>{t("about.cancel")}</AlertDialogCancel>
            <AlertDialogAction onClick={() => void install()}>
              {t("about.confirmInstall")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
      </CardContent>
    </Card>
  );
}

export default function About(props: Parameters<typeof AboutContent>[0]) {
  return (
    <LanguageBoundary>
      <AboutContent {...props} />
    </LanguageBoundary>
  );
}
