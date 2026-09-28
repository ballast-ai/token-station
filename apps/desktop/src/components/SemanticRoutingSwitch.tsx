import { useEffect, useId, useRef, useState } from "react";
import { getSemanticStatus, setSemanticEnabled, type SemanticStatus } from "../api";
import { useLocalizedCopy } from "./LanguageProvider";
import { Switch } from "./ui/switch";
import "./SemanticRoutingSwitch.css";

export default function SemanticRoutingSwitch() {
  const { copy } = useLocalizedCopy();
  const id = useId();
  const [status, setStatus] = useState<SemanticStatus | null>(null);
  const [pending, setPending] = useState(false);
  const [saveFailed, setSaveFailed] = useState(false);
  const [readFailed, setReadFailed] = useState(false);
  const mounted = useRef(false);
  const mutating = useRef(false);
  const revision = useRef(0);

  useEffect(() => {
    let active = true;
    let polling = false;
    mounted.current = true;
    const poll = async () => {
      if (polling || mutating.current) return;
      polling = true;
      const startedAt = revision.current;
      try {
        const next = await getSemanticStatus();
        if (active && startedAt === revision.current) {
          setStatus(next);
          setReadFailed(false);
          if (!next.available) window.clearInterval(timer);
        }
      } catch {
        if (active && startedAt === revision.current) setReadFailed(true);
      } finally {
        polling = false;
      }
    };
    const timer = window.setInterval(() => void poll(), 2_000);
    void poll();
    return () => {
      active = false;
      mounted.current = false;
      window.clearInterval(timer);
    };
  }, []);

  const changeEnabled = async (enabled: boolean) => {
    if (mutating.current || !status?.available) return;
    mutating.current = true;
    revision.current += 1;
    setPending(true);
    setSaveFailed(false);
    try {
      const next = await setSemanticEnabled(enabled);
      if (mounted.current) {
        setStatus(next);
        setReadFailed(false);
      }
    } catch {
      if (mounted.current) setSaveFailed(true);
    } finally {
      mutating.current = false;
      if (mounted.current) setPending(false);
    }
  };

  if (!status?.available) return null;

  const hasError = saveFailed || readFailed || status.state === "error";
  const message = saveFailed
    ? copy("Setting was not saved. Try again.", "设置未保存，请重试。", "設定未儲存，請重試。", "設定を保存できませんでした。再試行してください。")
    : readFailed
      ? copy("Cannot read the current status. Try again shortly.", "暂时无法读取状态，请稍后重试。", "暫時無法讀取狀態，請稍後重試。", "現在の状態を取得できません。しばらくしてから再試行してください。")
      : status.state === "error"
        ? copy("Local classification is unavailable. Existing rules remain active.", "本地分档暂不可用，继续使用原有规则。", "本機分檔暫時無法使用，繼續使用原有規則。", "ローカル分類を利用できません。既存のルールを使用します。")
        : status.enabled && status.state === "preparing"
          ? copy("Preparing the model. Existing rules remain active.", "正在准备模型，暂用原有规则。", "正在準備模型，暫用原有規則。", "モデルを準備中です。既存のルールを使用します。")
          : status.enabled && status.state === "loading"
            ? copy("Loading the model. Existing rules remain active.", "正在加载模型，暂用原有规则。", "正在載入模型，暫用原有規則。", "モデルを読み込み中です。既存のルールを使用します。")
            : null;

  return (
    <div className="semantic-routing-switch">
      <div className="semantic-routing-switch-control">
        <Switch
          id={id}
          checked={status.enabled}
          disabled={pending}
          aria-busy={pending}
          aria-describedby={`${id}-note${message ? ` ${id}-state` : ""}`}
          onCheckedChange={(enabled) => void changeEnabled(enabled)}
        />
        <label htmlFor={id}>{copy("Local smart tiers", "本地智能分档", "本機智慧分檔", "ローカルスマート分層")}</label>
      </div>
      <p className="semantic-routing-switch-note" id={`${id}-note`}>
        {copy("Off releases model memory and uses the original routing rules.", "关闭后释放模型内存，使用原有路由规则。", "關閉後釋放模型記憶體，使用原有路由規則。", "オフにするとモデルのメモリを解放し、既存のルールを使用します。")}
      </p>
      {message && <p className={`semantic-routing-switch-state${hasError ? " is-error" : ""}`} id={`${id}-state`} role={hasError ? "alert" : "status"}>{message}</p>}
    </div>
  );
}
