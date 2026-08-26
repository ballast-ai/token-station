import { useEffect, useRef, useState } from "react";
import {
  checkDesktopUpdate,
  installDesktopUpdateAndRestart,
  recordFrontendDiagnostic,
  type DesktopUpdateView,
  type RecoveryState,
} from "../api";
import { diagnosticInput } from "../diagnostics";
import { humanizeAppError } from "../errors";
import { useLocalizedCopy } from "./LanguageProvider";
import TokenStationMark from "./TokenStationMark";
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
  const { copy, language } = useLocalizedCopy();
  const languageRef = useRef(language);
  languageRef.current = language;
  const [busy, setBusy] = useState(initialError ? "" : "check-update");
  const [error, setError] = useState("");
  const [upgrade, setUpgrade] = useState<DesktopUpdateView | null>(null);
  const [confirmUpdate, setConfirmUpdate] = useState(false);
  const cancelUpdateRef = useRef<HTMLButtonElement>(null);

  const check = async () => {
    setBusy("check-update");
    setError("");
    try {
      setUpgrade(await checkDesktopUpdate());
    } catch (caught) {
      setError(humanizeAppError(caught, languageRef.current));
    } finally {
      setBusy("");
    }
  };

  useEffect(() => {
    if (initialError) {
      void recordFrontendDiagnostic(
        diagnosticInput("render_error", initialError),
      ).catch(() => undefined);
      return;
    }
    void check();
  // The startup condition is immutable for the lifetime of this screen.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [initialError]);

  const installUpdate = async () => {
    const expectedVersion = upgrade?.version;
    if (!expectedVersion) {
      const message = copy(
        "The selected update is no longer available. Check again.",
        "之前确认的更新已不可用，请重新检查。",
        "之前確認的更新已無法使用，請重新檢查。",
        "選択した更新は利用できなくなりました。もう一度確認してください。",
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
      if (!started) await check();
    } catch (caught) {
      const rawMessage = String(caught);
      if (requiresFreshUpdateCheck(rawMessage)) {
        setUpgrade((current) => invalidateUpdateCandidate(current, rawMessage));
      }
      setError(humanizeAppError(caught, languageRef.current));
      setBusy("");
    }
  };

  if (initialError) {
    return (
      <main className="recovery-shell">
        <section className="recovery-card" aria-live="polite">
          <TokenStationMark className="recovery-mark" size={42} />
          <div>
            <p className="eyebrow">TOKEN STATION</p>
            <h1>{copy("The interface failed to load", "界面加载失败", "介面載入失敗", "画面の読み込みに失敗しました")}</h1>
            <p className="sub">{copy(
              "Reload Token Station. If the problem continues, update to the latest version or contact support.",
              "请重新加载 Token Station。如果仍然失败，请更新到最新版本或联系支持。",
              "請重新載入 Token Station。如果仍然失敗，請更新至最新版本或聯絡支援。",
              "Token Station を再読み込みしてください。問題が続く場合は、最新版に更新するかサポートにお問い合わせください。",
            )}</p>
          </div>
          <div className="recovery-actions">
            <button className="btn primary" onClick={() => window.location.reload()}>
              {copy("Reload", "重新加载", "重新載入", "再読み込み")}
            </button>
          </div>
        </section>
      </main>
    );
  }

  const newerLocalData = initialState?.reason_code === "metrics_schema_newer";

  return (
    <main className="recovery-shell">
      <section className="recovery-card" aria-live="polite">
        <TokenStationMark className="recovery-mark" size={42} />
        <div>
          <p className="eyebrow">TOKEN STATION</p>
          <h1>{newerLocalData
            ? copy("Token Station needs an update", "需要更新 Token Station", "需要更新 Token Station", "Token Station の更新が必要です")
            : copy("Local data could not be opened", "无法打开本地数据", "無法開啟本機資料", "ローカルデータを開けません")}</h1>
          <p className="sub">{newerLocalData
            ? copy(
              "This local data was created by a newer version. Update to continue; your data will not be deleted.",
              "本机数据由较新版本创建。更新后即可继续使用，数据不会被删除。",
              "本機資料由較新版本建立。更新後即可繼續使用，資料不會被刪除。",
              "このローカルデータはより新しいバージョンで作成されました。更新後に続行でき、データは削除されません。",
            )
            : copy(
              "Restart Token Station. If the problem continues, install the latest version or contact support.",
              "请重启 Token Station。如果仍无法打开，请安装最新版本或联系支持。",
              "請重新啟動 Token Station。如果仍無法開啟，請安裝最新版本或聯絡支援。",
              "Token Station を再起動してください。開けない場合は、最新版をインストールするかサポートにお問い合わせください。",
            )}</p>
        </div>

        {error && <div className="banner err">{error}</div>}
        {upgrade?.status === "up_to_date" && (
          <div className="banner warn">{copy(
            `Version ${upgrade.current_version} is already the latest. Reinstall it or contact support if the problem continues.`,
            `当前已是最新版本 ${upgrade.current_version}。如果问题仍然存在，请重新安装或联系支持。`,
            `目前已是最新版本 ${upgrade.current_version}。如果問題仍然存在，請重新安裝或聯絡支援。`,
            `現在の ${upgrade.current_version} は最新版です。問題が続く場合は、再インストールするかサポートにお問い合わせください。`,
          )}</div>
        )}
        {(upgrade?.status === "unavailable" || upgrade?.status === "unsupported") && upgrade.message && (
          <div className="banner err">{humanizeAppError(upgrade.message, language)}</div>
        )}

        <div className="recovery-actions">
          {upgrade?.status === "update_available" && upgrade.version ? (
            <button className="btn primary" disabled={Boolean(busy)} onClick={() => setConfirmUpdate(true)}>
              {busy === "install-update"
                ? copy("Installing update…", "正在安装更新…", "正在安裝更新…", "更新をインストール中…")
                : copy(`Update to ${upgrade.version}`, `更新到 ${upgrade.version}`, `更新至 ${upgrade.version}`, `${upgrade.version} に更新`)}
            </button>
          ) : (
            <button className="btn primary" disabled={Boolean(busy)} onClick={() => void check()}>
              {busy === "check-update"
                ? copy("Checking for updates…", "正在检查更新…", "正在檢查更新…", "更新を確認中…")
                : copy("Check again", "重新检查", "重新檢查", "再確認")}
            </button>
          )}
          <button className="btn" onClick={() => window.location.reload()}>
            {copy("Restart screen", "重新加载", "重新載入", "再読み込み")}
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
              <AlertDialogTitle>{copy("Install app update?", "安装应用更新？", "安裝應用更新？", "アプリの更新をインストールしますか？")}</AlertDialogTitle>
              <AlertDialogDescription>{copy(
                "The signed update will be verified, installed, and then Token Station will restart.",
                "更新包将先验证签名，安装完成后 Token Station 会自动重启。",
                "更新套件會先驗證簽章，安裝完成後 Token Station 會自動重新啟動。",
                "署名を検証して更新をインストール後、Token Station を再起動します。",
              )}</AlertDialogDescription>
            </AlertDialogHeader>
            <AlertDialogFooter>
              <AlertDialogCancel ref={cancelUpdateRef}>{copy("Cancel", "取消", "取消", "キャンセル")}</AlertDialogCancel>
              <AlertDialogAction onClick={() => void installUpdate()}>
                {copy("Confirm update and restart", "确认更新并重启", "確認更新並重新啟動", "更新して再起動")}
              </AlertDialogAction>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>
      </section>
    </main>
  );
}
