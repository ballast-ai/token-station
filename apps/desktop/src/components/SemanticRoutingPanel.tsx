import { useEffect, useId, useRef, useState } from "react";
import {
  getSemanticStatus, prepareSemanticModel, setSemanticMode,
  type SemanticMode, type SemanticStatus,
} from "../api";
import { humanizeAppError } from "../errors";
import { useLocalizedCopy } from "./LanguageProvider";
import { Button } from "./ui/button";
import "./SemanticRoutingPanel.css";

export default function SemanticRoutingPanel() {
  const { copy, language } = useLocalizedCopy();
  const id = useId();
  const [status, setStatus] = useState<SemanticStatus | null>(null);
  const [error, setError] = useState<unknown>(null);
  const [pollError, setPollError] = useState<unknown>(null);
  const [pending, setPending] = useState(false);
  const mounted = useRef(false);
  const mutating = useRef(false);
  const revision = useRef(0);

  useEffect(() => {
    let active = true;
    let timer: ReturnType<typeof setTimeout> | undefined;
    mounted.current = true;
    const refresh = async () => {
      let available = true;
      const requestedRevision = revision.current;
      try {
        if (!mutating.current) {
          const next = await getSemanticStatus();
          if (active && requestedRevision === revision.current) {
            setStatus(next);
            setPollError(null);
            available = next.available;
          }
        }
      } catch (caught) {
        if (active && requestedRevision === revision.current) setPollError(caught);
      } finally {
        if (active && available) timer = setTimeout(() => void refresh(), 2000);
      }
    };
    void refresh();
    return () => {
      active = false;
      mounted.current = false;
      clearTimeout(timer);
    };
  }, []);

  async function change(action: () => Promise<SemanticStatus>) {
    if (mutating.current) return;
    mutating.current = true;
    revision.current += 1;
    setPending(true);
    setError(null);
    try {
      const next = await action();
      if (mounted.current) setStatus(next);
    } catch (caught) {
      if (mounted.current) setError(caught);
    } finally {
      mutating.current = false;
      if (mounted.current) setPending(false);
    }
  }

  if (!status?.available) return null;

  const states: Record<SemanticStatus["state"], string> = {
    unprepared: copy("Prepare the local model to start.", "先准备本地模型，再开启试验。", "先準備本機模型，再開啟試驗。", "開始するにはローカルモデルを準備してください。"),
    preparing: copy("Preparing the runtime and model…", "正在准备运行环境和模型…", "正在準備執行環境和模型…", "実行環境とモデルを準備中…"),
    loading: copy("Loading the model. Requests keep using existing rules.", "正在加载模型，请求继续使用原有规则。", "正在載入模型，請求繼續使用原有規則。", "モデルを読み込み中です。リクエストは既存ルールを使用します。"),
    ready: copy("Local model ready", "本地模型已就绪", "本機模型已就緒", "ローカルモデルの準備完了"),
    off: copy("Off · model memory released", "已关闭，模型内存已释放", "已關閉，模型記憶體已釋放", "無効・モデルのメモリを解放済み"),
    error: copy("The local classifier is unavailable. Existing rules remain active.", "本地分类器不可用，继续使用原有规则。", "本機分類器無法使用，繼續使用原有規則。", "ローカル分類器は利用できません。既存ルールを使用します。"),
  };
  const modes: { value: SemanticMode; label: string }[] = [
    { value: "off", label: copy("Off", "关闭", "關閉", "無効") },
    { value: "observe", label: copy("Observe", "仅观察", "僅觀察", "観察") },
    { value: "route", label: copy("Route", "参与路由", "參與路由", "ルーティングに使用") },
  ];
  const modeHelp = {
    off: copy("Existing routing stays active. Turning Off stops the model and releases its memory.", "使用原有路由。关闭会停止模型并释放内存。", "使用原有路由。關閉會停止模型並釋放記憶體。", "既存のルーティングを使用します。無効にするとモデルを停止し、メモリを解放します。"),
    observe: copy("Record suggested tiers and latency. Requests keep using existing routing rules.", "仅记录建议档位和耗时，请求仍按原有规则路由。", "僅記錄建議檔位和耗時，請求仍按原有規則路由。", "提案された段階と所要時間を記録します。リクエストは既存ルールで処理します。"),
    route: copy("Use SCX suggestions after model pins, user rules, and Agent hints. Failures fall back to existing rules.", "固定模型、用户规则和 Agent 提示优先，其余请求采用 SCX 建议；分类失败时使用原有规则。", "固定模型、使用者規則和 Agent 提示優先，其餘請求採用 SCX 建議；分類失敗時使用原有規則。", "モデル指定、ユーザールール、Agent ヒントを優先し、その後 SCX を使用します。失敗時は既存ルールに戻ります。"),
  };
  const visibleError = error ?? status.error ?? pollError;
  const preparing = status.state === "preparing";

  return (
    <section className="semantic-routing-panel" aria-labelledby={`${id}-title`}>
      <div className="semantic-routing-heading">
        <div>
          <h3 id={`${id}-title`}>{copy("SCX local tiers", "SCX 本地分档", "SCX 本機分檔", "SCX ローカル分類")}</h3>
          <p>{copy("Applies immediately to global and Agent routes that use smart tiers.", "立即应用于使用智能分档的全局及 Agent 路由。", "立即套用於使用智慧分檔的全域及 Agent 路由。", "スマート分層を使うグローバルおよび Agent のルートに即時適用します。")}</p>
        </div>
        <span className="count-badge">{copy("Experiment", "试验", "試驗", "実験")}</span>
      </div>
      <div className="semantic-routing-controls">
        <fieldset className="semantic-routing-modes" aria-describedby={`${id}-help`}>
          <legend className="sr-only">{copy("SCX mode", "SCX 模式", "SCX 模式", "SCX モード")}</legend>
          {modes.map(({ value, label }) => (
            <label key={value}>
              <input type="radio" name={`${id}-mode`} value={value} checked={status.mode === value}
                disabled={pending || (value !== "off" && (!status.model_ready || preparing))}
                onChange={() => void change(() => setSemanticMode(value))} />
              <span>{label}</span>
            </label>
          ))}
        </fieldset>
        {preparing ? (
          <Button type="button" variant="outline" disabled={pending}
            onClick={() => void change(() => setSemanticMode("off"))}>
            {copy("Cancel preparation", "取消准备", "取消準備", "準備をキャンセル")}
          </Button>
        ) : (!status.model_ready || status.state === "error") && (
          <Button type="button" variant="outline" disabled={pending}
            onClick={() => void change(prepareSemanticModel)}>
            {status.state === "error" ? copy("Prepare again", "重新准备", "重新準備", "再準備")
              : copy("Prepare local model", "准备本地模型", "準備本機模型", "ローカルモデルを準備")}
          </Button>
        )}
      </div>
      <p id={`${id}-help`} className="semantic-routing-help">{modeHelp[status.mode]}</p>
      <p className="semantic-routing-state" role="status">{states[status.state]}</p>
      {visibleError != null && <p className="semantic-routing-error" role="alert">{humanizeAppError(visibleError, language)}</p>}
      {!status.model_ready && <p className="semantic-routing-note">{copy("Preparation checks local files and downloads the runtime and model if needed.", "准备时检查本地文件，必要时下载运行环境和模型。", "準備時檢查本機檔案，必要時下載執行環境和模型。", "準備時にローカルファイルを確認し、必要に応じて実行環境とモデルをダウンロードします。")}</p>}
      <p className="semantic-routing-note">{copy("Multimodal, oversized text, and native server-tool requests keep using existing rules. The enabled model uses additional memory.", "多模态、过长文本和服务端原生工具请求继续使用原有规则。模型启用时会额外占用内存。", "多模態、過長文字和伺服器端原生工具請求繼續使用原有規則。模型啟用時會額外占用記憶體。", "マルチモーダル、長すぎるテキスト、サーバー側のネイティブツールのリクエストは既存ルールを使用します。モデルの有効時は追加のメモリを使用します。")}</p>
    </section>
  );
}
