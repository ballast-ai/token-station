import { useEffect, useRef, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Github } from "@lobehub/icons";
import {
  BookOpen,
  ExternalLink,
  Loader2,
  MousePointerClick,
  Network,
  RefreshCw,
  ShieldCheck,
} from "lucide-react";
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
import { Badge } from "../components/ui/badge";
import {
  Card,
  CardContent,
  CardHeader,
} from "../components/ui/card";
import { Progress } from "../components/ui/progress";
import { Separator } from "../components/ui/separator";
import { humanizeAppError } from "../errors";
import { useErrorToast } from "../components/ErrorToast";
import TokenStationMark from "../components/TokenStationMark";

const PROJECT_URL = "https://github.com/ballast-ai/token-station";
const RELEASES_URL = `${PROJECT_URL}/releases`;

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
  const { showError } = useErrorToast();
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
    }).catch((caught) => {
      if (!disposed) {
        showError(
          humanizeAppError(caught, language),
          "desktop-update-progress-listener",
        );
      }
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [language, showError]);

  const check = async () => {
    setBusy("checking");
    setErr("");
    setUpdate(null);
    try {
      const next = await checkDesktopUpdate();
      setUpdate(next);
      if (next.status === "update_available" && next.version) {
        setConfirmOpen(true);
      }
    } catch (e) {
      showError(humanizeAppError(e, language), "desktop-update-check");
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
        showError(message, "desktop-update-install");
      }
      setBusy("");
    }
  };

  const copyReleaseUrl = async (url: string) => {
    try {
      await navigator.clipboard.writeText(url);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1_500);
    } catch {
      showError(
        copy(
          "Could not copy the release link. Check the system clipboard permission and try again.",
          "无法复制发布链接。请检查系统剪贴板权限，然后重试。",
        ),
        "copy-release-url",
      );
    }
  };

  const openExternal = async (url: string) => {
    try {
      await openUrl(url);
    } catch (caught) {
      showError(
        humanizeAppError({ code: "open_external_failed", detail: caught }, language),
        `about-open:${url}`,
      );
    }
  };

  const progressPercent = progress?.total && progress.total > 0
    ? Math.min(100, Math.floor((progress.downloaded / progress.total) * 100))
    : null;

  return (
    <section className="about-page">
      <header className="about-page-head panel-head">
        <h2>{t("about.title")}</h2>
        <p className="sub">{t("about.description")}</p>
      </header>

      <Card className="about-product-card">
        <CardHeader className="about-product-head">
          <div className="about-product-identity">
            <span className="about-product-mark">
              <TokenStationMark size={42} />
            </span>
            <div className="about-product-copy">
              <h3>Token Station</h3>
              <p>{t("about.productDescription")}</p>
              <div className="about-version-badges" aria-label={t("about.versionGroup") }>
                <Badge variant="outline" aria-label={`Desktop ${desktopVersion}`}>
                  <span>Desktop</span>
                  <strong className="mono">{desktopVersion}</strong>
                </Badge>
                <Badge variant="outline" aria-label={`Core ${coreVersion}`}>
                  <span>Core</span>
                  <strong className="mono">{coreVersion}</strong>
                </Badge>
              </div>
            </div>
          </div>

          <div className="about-product-actions">
            <Button variant="outline" type="button" onClick={() => void openExternal(PROJECT_URL)}>
              <Github size={16} aria-hidden="true" />
              {t("about.source")}
            </Button>
            <Button variant="outline" type="button" onClick={() => void openExternal(RELEASES_URL)}>
              <ExternalLink aria-hidden="true" />
              {t("about.releases")}
            </Button>
            <Button
              disabled={Boolean(busy)}
              onClick={update?.status === "update_available" ? () => setConfirmOpen(true) : check}
            >
              {busy === "checking" || busy === "installing" ? (
                <Loader2 className="about-spin" aria-hidden="true" />
              ) : (
                <RefreshCw aria-hidden="true" />
              )}
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
          </div>
        </CardHeader>

        {(err || update || (busy === "installing" && progress)) && <Separator />}

        {(err || update || (busy === "installing" && progress)) && (
          <CardContent className="about-update-content">
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
                    <Button variant="outline" size="sm" onClick={() => void copyReleaseUrl(update.release_url)}>
                      {copied ? t("about.copied") : t("about.copyLink")}
                    </Button>
                  </span>
                )}
              </div>
            )}

            {busy === "installing" && progress && (
              <div className="about-download-progress" aria-live="polite">
                {progressPercent !== null ? (
                  <>
                    <span>{t("about.downloaded", { percent: progressPercent })}</span>
                    <Progress
                      value={progressPercent}
                      role="progressbar"
                      aria-label={t("about.downloadProgress")}
                      aria-valuemin={0}
                      aria-valuemax={100}
                      aria-valuenow={progressPercent}
                    />
                  </>
                ) : (
                  <span>{t("about.downloadedBytes", { downloaded: progress.downloaded })}</span>
                )}
              </div>
            )}
          </CardContent>
        )}
      </Card>

      <Card className="about-trust-card">
        <CardHeader className="about-trust-head">
          <div>
            <h3>{t("about.trustTitle")}</h3>
            <p>{t("about.trustDescription")}</p>
          </div>
          {onOpenFirstRunGuide && (
            <Button variant="outline" type="button" onClick={onOpenFirstRunGuide}>
              <BookOpen aria-hidden="true" />
              {copy("Review getting started guide", "重新查看新手引导")}
            </Button>
          )}
        </CardHeader>
        <Separator />
        <CardContent className="about-trust-grid">
          <div className="about-trust-item">
            <MousePointerClick aria-hidden="true" />
            <div>
              <strong>{t("about.onDemandTitle")}</strong>
              <span>{t("about.onDemandDescription")}</span>
            </div>
          </div>
          <div className="about-trust-item">
            <ShieldCheck aria-hidden="true" />
            <div>
              <strong>{t("about.signedTitle")}</strong>
              <span>{t("about.signedDescription")}</span>
            </div>
          </div>
          <div className="about-trust-item">
            <Network aria-hidden="true" />
            <div>
              <strong>{t("about.gatewayTitle")}</strong>
              <span>{t("about.gatewayDescription")}</span>
            </div>
          </div>
        </CardContent>
      </Card>

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
