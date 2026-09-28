import { useEffect, useId, useRef, useState } from "react";
import {
  getSemanticStatus, prepareSemanticModel, setSemanticMode,
  type SemanticMode, type SemanticStatus,
} from "../api";
import { humanizeAppError } from "../errors";
import { useLocalizedCopy, type LocalizedCopy } from "./LanguageProvider";
import { Button } from "./ui/button";
import "./SemanticRoutingPanel.css";

function tierLabel(tier: string | null, copy: LocalizedCopy): string {
  switch (tier) {
    case "low": case "tier_low": return copy("Low", "低档", "低檔", "低");
    case "medium": case "mid": case "tier_mid": return copy("Medium", "中档", "中檔", "中");
    case "high": case "tier_high": return copy("High", "高档", "高檔", "高");
    default: return copy("No suggestion", "无建议", "無建議", "提案なし");
  }
}

function outcomeLabel(outcome: string, copy: LocalizedCopy): string {
  switch (outcome) {
    case "observed": return copy("Observed only", "仅观察", "僅觀察", "観察のみ");
    case "applied": return copy("SCX applied", "已采用 SCX", "已採用 SCX", "SCX を適用");
    case "overridden": return copy("Existing rule takes priority", "已有规则优先", "既有規則優先", "既存ルールを優先");
    case "timeout": return copy("Timed out · existing rules", "超时，使用原有规则", "逾時，使用原有規則", "タイムアウト・既存ルールを使用");
    case "busy": return copy("Classifier busy · existing rules", "分类器忙，使用原有规则", "分類器忙碌，使用原有規則", "分類器が使用中・既存ルールを使用");
    case "loading": return copy("Loading · existing rules", "加载中，使用原有规则", "載入中，使用原有規則", "読み込み中・既存ルールを使用");
    case "unavailable": return copy("Not ready · existing rules", "未就绪，使用原有规则", "尚未就緒，使用原有規則", "準備未完了・既存ルールを使用");
    case "unsupported": return copy("Unsupported input · existing rules", "不支持此输入，使用原有规则", "不支援此輸入，使用原有規則", "未対応の入力・既存ルールを使用");
    case "invalid": return copy("Invalid result · existing rules", "结果无效，使用原有规则", "結果無效，使用原有規則", "無効な結果・既存ルールを使用");
    case "cancelled": return copy("Cancelled · existing rules", "已取消，使用原有规则", "已取消，使用原有規則", "キャンセル済み・既存ルールを使用");
    case "no_route": return copy("No eligible route", "没有可用路由", "沒有可用路由", "利用可能なルートなし");
    default: return copy("Classification failed · existing rules", "分类失败，使用原有规则", "分類失敗，使用原有規則", "分類失敗・既存ルールを使用");
  }
}

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
  const rows = [...status.observations].sort((a, b) => b.id - a.id).slice(0, 10);
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
      <div className="semantic-routing-counts">
        <span>{copy("Classified", "已分类", "已分類", "分類済み")} <strong>{status.counts.classified.toLocaleString(language)}</strong></span>
        <span>{copy("Different tiers", "档位不同", "檔位不同", "段階の相違")} <strong>{status.counts.disagreements.toLocaleString(language)}</strong></span>
        <span>{copy("Fallbacks", "回退次数", "備援次數", "フォールバック")} <strong>{status.counts.fallbacks.toLocaleString(language)}</strong></span>
      </div>
      {rows.length > 0 ? (
        <div className="semantic-routing-comparisons" tabIndex={0} role="region" aria-label={copy("Recent tier comparisons", "最近分档对照", "最近分檔對照", "最近の分類比較")}>
          <table aria-label={copy("Recent tier comparisons", "最近分档对照", "最近分檔對照", "最近の分類比較")}>
            <thead><tr>
              <th scope="col">{copy("Request", "请求", "請求", "リクエスト")}</th>
              <th scope="col">{copy("Existing rules", "原有规则", "原有規則", "既存ルール")}</th>
              <th scope="col">{copy("SCX suggestion", "SCX 建议", "SCX 建議", "SCX の提案")}</th>
              <th scope="col">{copy("Classification time", "分类耗时", "分類耗時", "分類時間")}</th>
              <th scope="col">{copy("Outcome", "处理结果", "處理結果", "処理結果")}</th>
            </tr></thead>
            <tbody>{rows.map((row) => (
              <tr key={row.id} data-different={row.baseline_tier != null && row.suggested_tier != null && row.baseline_tier !== row.suggested_tier}>
                <td className="semantic-routing-number">#{row.id}</td>
                <td>{tierLabel(row.baseline_tier, copy)}</td>
                <td><strong>{tierLabel(row.suggested_tier, copy)}</strong></td>
                <td className="semantic-routing-number">{row.latency_ms == null ? "—" : `${row.latency_ms.toLocaleString(language, { maximumFractionDigits: 0 })} ms`}</td>
                <td>{outcomeLabel(row.outcome, copy)}</td>
              </tr>
            ))}</tbody>
          </table>
        </div>
      ) : <p className="semantic-routing-note">{copy("Tier comparisons appear after eligible requests in Observe or Route mode.", "开启观察或路由后，符合条件的请求会显示在这里。", "開啟觀察或路由後，符合條件的請求會顯示在這裡。", "観察またはルーティングを有効にすると、対象リクエストの比較を表示します。")}</p>}
      <p className="semantic-routing-note">{copy("Classification runs locally. Comparison records contain no request text.", "分类在本机运行，对照记录不保存请求正文。", "分類在本機執行，對照記錄不儲存請求正文。", "分類はローカルで実行します。比較記録にリクエスト本文は保存しません。")}</p>
    </section>
  );
}
