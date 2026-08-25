import { useCallback, useEffect, useRef, useState } from "react";
import {
  getRecentReceipts,
  type ReceiptConversionView,
  type ReceiptDecidedByView,
  type ReceiptFeaturesView,
  type ReceiptRouteView,
  type ReceiptView,
} from "../api";
import { humanizeAppError, humanizeReceiptError } from "../errors";
import { useLocalizedCopy, type LocalizedCopy } from "./LanguageProvider";
import { useErrorToast } from "./ErrorToast";

const MAX_RECEIPTS = 5;
const AUTO_REFRESH_MS = 10_000;

function formatTime(timestamp: number, locale: string): string {
  return new Date(timestamp).toLocaleString(locale, {
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    hour12: false,
  });
}

function formatUpdatedAt(timestamp: number, locale: string): string {
  return new Date(timestamp).toLocaleTimeString(locale, {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    hour12: false,
  });
}

function formatRoute(route: ReceiptRouteView | null, noRoute: string): string {
  return route ? `${route.upstream}/${route.model}` : noRoute;
}

function formatCost(
  receipt: ReceiptView,
  actual: string,
  estimated: string,
  unknown: string,
): string {
  if (receipt.cost_kind === "unknown" || receipt.cost_micros == null) return unknown;
  const amount = (receipt.cost_micros / 1_000_000).toFixed(6);
  return receipt.cost_kind === "actual" ? `${actual} ${amount}` : `${estimated} ${amount}`;
}

function formatTokens(
  receipt: ReceiptView,
  unknown: string,
  reported: (total: number) => string,
): string {
  if (!receipt.usage) return unknown;
  const total = receipt.usage.input_tokens + receipt.usage.output_tokens;
  return receipt.usage_semantics === "provider_reported_v1"
    ? reported(total)
    : `${total} tokens`;
}

function formatDecisionReason(
  reason: ReceiptDecidedByView,
  copy: LocalizedCopy,
): string {
  switch (reason.tier) {
    case "rule":
      return copy(`Rule · ${reason.rule}`, `规则 · ${reason.rule}`, `規則 · ${reason.rule}`, `ルール · ${reason.rule}`);
    case "hint":
      return copy(`Hint · ${reason.kind}/${reason.value}`, `提示 · ${reason.kind}/${reason.value}`, `提示 · ${reason.kind}/${reason.value}`, `ヒント · ${reason.kind}/${reason.value}`);
    case "heuristic":
      return copy(
        `Heuristic score ${reason.score} · matched band ≥ ${reason.matched_band_at_least}`,
        `启发式评分 ${reason.score} · 命中档位下界 ≥ ${reason.matched_band_at_least}`, `啟發式評分 ${reason.score} · 命中檔位下界 ≥ ${reason.matched_band_at_least}`, `ヒューリスティックスコア ${reason.score} · 命中バンド下限 ≥ ${reason.matched_band_at_least}`
      );
    case "exact_model":
      return copy(`Exact model · ${reason.model}`, `指定模型 · ${reason.model}`, `指定模型 · ${reason.model}`, `指定モデル · ${reason.model}`);
    case "quota":
      return copy("Quota-first", "额度优先", "額度優先", "クォータ優先");
    default:
      return copy("Default route", "默认路由", "預設路由", "デフォルトルーティング");
  }
}

/** One-line quota decision summary: remaining quota, time to reset, rate headroom, and state. */
function formatQuotaDecision(
  quota: NonNullable<ReceiptRouteView["quota"]>,
  copy: LocalizedCopy,
): string {
  const parts: string[] = [];
  if (quota.remaining_permille != null) {
    parts.push(copy(
      `${(quota.remaining_permille / 10).toFixed(0)}% left`,
      `剩 ${(quota.remaining_permille / 10).toFixed(0)}%`, `剩 ${(quota.remaining_permille / 10).toFixed(0)}%`, `残り ${(quota.remaining_permille / 10).toFixed(0)}%`
    ));
  }
  const NO_RESET_MS = 100 * 24 * 60 * 60 * 1000;
  if (quota.reset_ms != null && quota.reset_ms > 0 && quota.reset_ms < NO_RESET_MS) {
    const minutes = Math.round(quota.reset_ms / 60000);
    parts.push(copy(`resets in ${minutes}m`, `${minutes}分钟后刷新`, `${minutes}分鐘後重新整理`, `${minutes}分後にリセット`));
  }
  parts.push(copy(
    `headroom ${(quota.headroom_permille / 10).toFixed(0)}%`,
    `速率余量 ${(quota.headroom_permille / 10).toFixed(0)}%`, `速率餘量 ${(quota.headroom_permille / 10).toFixed(0)}%`, `スループット余力 ${(quota.headroom_permille / 10).toFixed(0)}%`
  ));
  if (quota.exhausted) parts.push(copy("exhausted", "已耗尽", "已耗盡", "使用完了"));
  else if (quota.pressured) parts.push(copy("pressured", "速率吃紧", "壓力", "プレッシャー"));
  return parts.join(" · ");
}

function formatFeatures(
  features: ReceiptFeaturesView,
  copy: LocalizedCopy,
): string {
  const flags = [
    features.tool_count ? copy(
      `${features.tool_count} tools`,
      `${features.tool_count} 个工具`,
      `${features.tool_count} 個工具`,
      `${features.tool_count} 個のツール`,
    ) : null,
    features.has_images ? copy("images", "图像", "影像", "画像") : null,
    features.requires_json_schema ? "JSON Schema" : null,
    features.code_block_count ? copy(
      `${features.code_block_count} code blocks`,
      `${features.code_block_count} 个代码块`,
      `${features.code_block_count} 個程式碼區塊`,
      `${features.code_block_count} 個のコードブロック`,
    ) : null,
  ].filter(Boolean);
  return copy(
    `${features.estimated_input_tokens} estimated input tokens · ${features.message_count} messages${flags.length ? ` · ${flags.join(" · ")}` : ""}`,
    `${features.estimated_input_tokens} 估算输入 token · ${features.message_count} 条消息${flags.length ? ` · ${flags.join(" · ")}` : ""}`, `${features.estimated_input_tokens} 估算輸入 token · ${features.message_count} 個訊息${flags.length ? ` · ${flags.join(" · ")}` : ""}`, `${features.estimated_input_tokens} 予測入力トークン · ${features.message_count} 件のメッセージ${flags.length ? ` · ${flags.join(" · ")}` : ""}`
  );
}

function formatConversionStage(
  stage: ReceiptConversionView["stage"],
  copy: LocalizedCopy,
): string {
  switch (stage) {
    case "inbound_normalize":
      return copy("Receive client request", "收到调用方请求", "接收客戶端請求", "クライアントリクエストを受信");
    case "provider_request":
      return copy("Convert to provider format", "转为供应商格式", "轉為供應商格式", "プロバイダー形式に変換");
    case "provider_response":
      return copy("Parse provider response", "解析供应商响应", "解析供應商回應", "プロバイダーレスポンスを解析");
    case "outbound_render":
      return copy("Return in client format", "返回调用方格式", "返回客戶端格式", "クライアント形式で返す");
    case "stream_translate":
      return copy("Translate streaming chunks", "转换流式片段", "轉換流式片段", "ストリーミングチャンクを変換");
    default:
      return stage;
  }
}

export function ReceiptDetails({ receipt }: { receipt: ReceiptView }) {
  const { language, copy } = useLocalizedCopy();
  const attempts = receipt.attempt_records ?? [];
  const conversions = receipt.conversion_reports ?? [];
  const diagnosis = humanizeReceiptError(receipt, language);
  const stoppedDuringInbound = receipt.decision == null
    && receipt.attempt_records.length === 0
    && receipt.conversion_reports.some(
      (conversion) => conversion.stage === "inbound_normalize" && !conversion.succeeded,
    );
  return (
    <div className="receipt-timeline receipt-timeline-compact" data-testid="receipt-trace">
      {receipt.usage && receipt.usage_semantics === "provider_reported_v1" && (
        <p className="receipt-section-empty">
          {copy(
            "Historical provider-reported input may exclude cache tokens; this total is not canonical.",
            "历史上游输入可能不含缓存 Token；此总量并非规范总量。",
            "歷史上游輸入可能不含快取 Token；此總量並非規範總量。",
            "過去のプロバイダー報告入力にはキャッシュトークンが含まれない場合があり、この合計は正規化済みではありません。",
          )}
        </p>
      )}
      {diagnosis && (
        <section className="receipt-diagnosis" aria-label={copy("Error diagnosis", "错误诊断", "錯誤診斷", "エラー診断")}>
          <h4>{copy("Diagnosis", "诊断", "診斷", "診断")}</h4>
          <strong>{diagnosis.layer}</strong>
          <span>{diagnosis.message}</span>
          <small>{copy(`Next: ${diagnosis.suggestion}`, `下一步：${diagnosis.suggestion}`, `下一步：${diagnosis.suggestion}`, `次：${diagnosis.suggestion}`)}</small>
        </section>
      )}
      <section aria-label={copy("Decision record", "决策记录", "決策紀錄", "決定記録")}>
        <h4>{copy("Decision", "路由决策", "決策", "決定")}</h4>
        {receipt.decision ? (
          <div className="receipt-event">
            <span className="receipt-event-index">D</span>
            <div>
              <strong>{formatRoute(receipt.decision, copy("No route", "未产生路由", "未產生路由", "ルーティングが生成されませんでした"))}</strong>
              <span>
                {receipt.decision.pool} · {formatDecisionReason(receipt.decision.decided_by, copy)} · {copy(
                  `${receipt.decision.fallbacks} fallback candidates`,
                  `${receipt.decision.fallbacks} 个候选回退`, `${receipt.decision.fallbacks} 個候補回退`, `${receipt.decision.fallbacks} 個の候補フォールバック`
                )}
              </span>
              <small>{formatFeatures(receipt.decision.features, copy)}</small>
              {receipt.decision.quota && (
                <small className="receipt-quota-line">
                  {formatQuotaDecision(receipt.decision.quota, copy)}
                </small>
              )}
            </div>
          </div>
        ) : (
          <p className="receipt-section-empty">{stoppedDuringInbound
            ? copy(
              "The request stopped during local inbound conversion, before routing.",
              "请求在本地入站转换阶段停止，尚未进入路由。", "請求在本地入站轉換階段停止，尚未進入路由。", "リクエストはローカルインバウンド変換段階で停止し、ルーティングに入る前に終了しました。"
            )
            : copy("No decision record", "没有决策记录", "沒有決策記錄", "決定記録がありません")}</p>
        )}
      </section>

      <section aria-label={copy("Upstream attempt records", "上游尝试记录", "上游嘗試記錄", "アップストリームの試行記録")}>
        <h4>{copy("Attempts", "上游尝试", "嘗試", "試行")}</h4>
        {attempts.length ? attempts.map((attempt) => (
          <div className="receipt-event" key={attempt.ordinal}>
            <span className="receipt-event-index">{attempt.ordinal}</span>
            <div>
              <strong>{attempt.upstream}/{attempt.model}</strong>
              <span>
                {attempt.http_status
                  ? `HTTP ${attempt.http_status}`
                  : attempt.error_code ?? copy("No final HTTP status", "无 HTTP 终态", "無最終 HTTP 狀態", "最終 HTTP 状態がない")} · {attempt.latency_ms} ms
              </span>
              <small>
                {attempt.stream_outcome ?? copy("Non-streaming", "非流式", "非流式", "非ストリーム")} · {attempt.fallback_allowed
                  ? copy("Fallback allowed", "允许回退", "允許回退", "フォールバックを許可")
                  : copy("Fallback blocked", "不允许回退", "不允許回退", "フォールバックを禁止")}
              </small>
            </div>
          </div>
        )) : <p className="receipt-section-empty">{stoppedDuringInbound
          ? copy(
            "The request was not sent upstream because local conversion failed.",
            "本地转换失败，请求未发往上游。", "因為本地轉換失敗，請求未發往上游。", "ローカルの変換に失敗したため、リクエストがアップストリームに送信されませんでした。"
          )
          : copy("No upstream attempts", "没有真实上游尝试", "沒有真實上游嘗試", "実際のアップストリームの試行がありません")}</p>}
      </section>

      <section className="receipt-conversions" aria-label={copy("Protocol conversion records", "协议转换记录", "協議轉換記錄", "プロトコル変換記録")}>
        <div className="receipt-conversion-heading">
          <h4>{copy("Protocol flow", "协议转换", "協議流", "プロトコルフロー")}</h4>
          <p>{copy(
            "Client request to provider and back to the client",
            "调用方请求经 Token Station 转给供应商，再转换后返回", "客戶端請求經 Token Station 轉給供應商，再轉換後返回", "クライアントのリクエストが Token Station を経由してプロバイダーに送信され、変換後クライアントに返されます"
          )}</p>
        </div>
        {conversions.length ? (
          <ol
            className="receipt-conversion-flow"
            aria-label={copy("Protocol conversion flow", "协议转换流程", "協議轉換流程", "プロトコル変換フロー")}
          >
            {conversions.map((conversion) => (
              <li className="receipt-conversion-step" key={conversion.ordinal}>
                <span className="receipt-event-index">{conversion.ordinal}</span>
                <div>
                  <strong>{formatConversionStage(conversion.stage, copy)}</strong>
                  <code>{conversion.stage}</code>
                  <span>{conversion.source_protocol} → {conversion.target_protocol}</span>
                  <small>{conversion.outcome === "cancelled"
                    ? copy("Cancelled by client", "客户端已取消", "客戶端已取消", "クライアントがキャンセルしました")
                    : conversion.succeeded
                      ? copy("Succeeded", "成功", "成功", "成功")
                      : [conversion.error_code, conversion.reason_code, conversion.reason_detail]
                        .filter(Boolean)
                        .join(" · ") || copy("Failed", "失败", "失敗", "失敗")}</small>
                </div>
              </li>
            ))}
          </ol>
        ) : <p className="receipt-section-empty">{copy("No conversion records", "没有转换记录", "沒有轉換記錄", "変換記録がありません")}</p>}
      </section>
    </div>
  );
}

function CopyRequestId({ requestId }: { requestId: string }) {
  const { copy } = useLocalizedCopy();
  const { showError } = useErrorToast();
  const [copied, setCopied] = useState(false);
  const copyRequestId = async () => {
    try {
      if (!navigator.clipboard) throw new Error("clipboard unavailable");
      await navigator.clipboard.writeText(requestId);
      setCopied(true);
    } catch {
      showError(
        copy(
          "Could not copy the request ID. Check the system clipboard permission and try again.",
          "无法复制请求 ID。请检查系统剪贴板权限，然后重试。", "無法複製請求 ID。請檢查系統剪貼簿許可權，然後重試。", "リクエスト ID をコピーできませんでした。システムクリップボードの権限を確認し、再度お試しください。"
        ),
        `copy-request-id:${requestId}`,
      );
    }
  };
  return (
    <button type="button" className="btn ghost tiny receipt-copy" onClick={() => void copyRequestId()}>
      {copied ? copy("Copied", "已复制", "已複製", "コピーしました") : copy("Copy request ID", "复制请求 ID", "複製請求 ID", "リクエスト ID をコピー")}
    </button>
  );
}

function RefreshIcon() {
  return (
    <svg
      data-icon="inline-start"
      viewBox="0 0 16 16"
      aria-hidden="true"
    >
      <path d="M13.4 5.8A5.8 5.8 0 1 0 13 10.6" />
      <path d="M13.5 2.8v3.4h-3.4" />
    </svg>
  );
}

export default function RecentReceipts() {
  const { language, copy } = useLocalizedCopy();
  const { showError } = useErrorToast();
  const [receipts, setReceipts] = useState<ReceiptView[] | null>(null);
  const [error, setError] = useState("");
  const [refreshing, setRefreshing] = useState(false);
  const [lastUpdatedAt, setLastUpdatedAt] = useState<number | null>(null);
  const mounted = useRef(true);
  const inFlight = useRef(false);
  const queued = useRef(false);
  const latestLoader = useRef<(background?: boolean) => Promise<void>>(async () => undefined);

  const loadReceipts = useCallback(async (background = false) => {
    if (inFlight.current) {
      queued.current = true;
      return;
    }
    inFlight.current = true;
    if (background) setRefreshing(true);
    setError("");
    try {
      const result = await getRecentReceipts(MAX_RECEIPTS);
      if (mounted.current) {
        const next = result.slice(0, MAX_RECEIPTS);
        setReceipts((current) => {
          if (
            current
            && current.length === next.length
            && current.every((receipt, index) => receipt.request_id === next[index]?.request_id)
          ) {
            return current;
          }
          return next;
        });
        setLastUpdatedAt(Date.now());
      }
    } catch (caught) {
      if (mounted.current) {
        const message = humanizeAppError(caught);
        if (background) showError(message, "recent-receipts-refresh");
        else setError(message);
      }
    } finally {
      inFlight.current = false;
      if (
        mounted.current
        && queued.current
        && document.visibilityState === "visible"
      ) {
        queued.current = false;
        queueMicrotask(() => {
          if (mounted.current && document.visibilityState === "visible") {
            void latestLoader.current(true);
          }
        });
      } else {
        queued.current = false;
        if (mounted.current) setRefreshing(false);
      }
    }
  }, [showError]);
  latestLoader.current = loadReceipts;

  useEffect(() => {
    mounted.current = true;
    void loadReceipts();
    return () => {
      mounted.current = false;
      queued.current = false;
    };
  }, [loadReceipts]);

  useEffect(() => {
    const refreshVisiblePage = () => {
      if (document.visibilityState === "visible") void loadReceipts(true);
    };
    const timer = window.setInterval(refreshVisiblePage, AUTO_REFRESH_MS);
    window.addEventListener("focus", refreshVisiblePage);
    return () => {
      window.clearInterval(timer);
      window.removeEventListener("focus", refreshVisiblePage);
    };
  }, [loadReceipts]);

  const busy = refreshing || (receipts === null && !error);

  return (
    <section className="panel receipt-panel" aria-labelledby="recent-receipts-heading">
      <div className="panel-head split-heading">
        <div>
          <h2 id="recent-receipts-heading">{copy("Recent requests", "最近 5 次请求", "最近 5 次請求", "最近の 5 件のリクエスト")}</h2>
          <p className="sub">{copy(
            "Receipts retain route, final status, and usage metadata. Plaintext bodies are stored separately for 7 days by default.",
            "Receipt 保留路由、终态与用量元数据；正文明文独立存储，默认保留 7 天。", "回執會保留路由、最終狀態與用量中繼資料；純文字內文會獨立儲存，預設保留 7 天。", "レシートにはルーティング、最終状態、使用状況のメタデータが保持されます。平文の本文は別途保存され、既定では7日間保持されます。"
          )}</p>
        </div>
        <div className="receipt-refresh-actions">
          <span className="receipt-refresh-note">
            {copy("Refreshes every 10 seconds", "每 10 秒自动刷新", "每 10 秒自動重新整理", "10 秒ごとに自動更新")}
            {lastUpdatedAt != null && (
              <>
                {" · "}
                <time
                  data-testid="receipt-updated-at"
                  dateTime={new Date(lastUpdatedAt).toISOString()}
                >
                  {copy(
                    `${formatUpdatedAt(lastUpdatedAt, language)} updated`,
                    `${formatUpdatedAt(lastUpdatedAt, language)} 更新`, `${formatUpdatedAt(lastUpdatedAt, language)} 更新`, `${formatUpdatedAt(lastUpdatedAt, language)} を更新`
                  )}
                </time>
              </>
            )}
          </span>
          {receipts && receipts.length > 0 && (
            <span className="count-badge">{copy(
              `${receipts.length} records`,
              `${receipts.length} 条`, `${receipts.length} 筆`, `${receipts.length} 件`
            )}</span>
          )}
          <button
            type="button"
            className={`btn receipt-refresh-button${busy ? " busy" : ""}`}
            aria-label={copy("Refresh recent requests", "刷新最近请求", "重新整理最近請求", "最近のリクエストを更新")}
            aria-busy={busy}
            disabled={busy}
            onClick={() => void loadReceipts(true)}
          >
            <RefreshIcon />
            <span>{busy
              ? copy("Refreshing…", "刷新中…", "重新整理中…", "更新中…")
              : copy("Refresh", "刷新", "重新整理", "更新")}</span>
          </button>
        </div>
      </div>

      {!receipts && !error && (
        <div className="receipt-state" role="status">{copy(
          "Loading request receipts…",
          "正在读取请求回执…", "正在讀取請求回執…", "リクエストのレシートを読み込んでいます…"
        )}</div>
      )}
      {error && receipts === null && (
        <div className="receipt-state error-text" role="alert">
          {copy(`Failed to load request receipts: ${error}`, `请求回执读取失败：${error}`, `請求回執讀取失敗：${error}`, `リクエストのレシートを読み込めませんでした：${error}`)}
        </div>
      )}
      {receipts?.length === 0 && (
        <div className="empty-state">
          <strong>{copy("No request receipts yet", "还没有请求回执", "還沒有請求回執", "リクエストのレシートはまだありません")}</strong>
          <span>{copy(
            "Start the proxy and complete a request to create a receipt.",
            "启动代理并完成一次请求后，这里会显示请求回执。", "啟動代理並完成一次請求後，這裡會顯示請求回執。", "プロキシを起動してリクエストを1回完了すると、ここにリクエストのレシートが表示されます。"
          )}</span>
        </div>
      )}

      {receipts && receipts.length > 0 && (
        <div className="receipt-list">
          {receipts.map((receipt) => {
            const ok = receipt.status >= 200 && receipt.status < 400 && !receipt.error_code;
            return (
              <details className="receipt-row" data-testid="receipt-row" key={receipt.request_id}>
                <summary>
                  <time dateTime={new Date(receipt.started_at_ms).toISOString()}>
                    {formatTime(receipt.started_at_ms, language)}
                  </time>
                  <span className="receipt-agent">
                    {receipt.agent_id ?? copy("Unknown Agent", "未知 Agent", "未知 Agent", "不明な Agent")} · {receipt.protocol}
                  </span>
                  <strong className="receipt-route">
                    {formatRoute(receipt.routing, copy("No route", "未产生路由", "未產生路由", "ルーティングが生成されませんでした"))}
                  </strong>
                  <span className={`receipt-status ${ok ? "success" : "failure"}`}>
                    HTTP {receipt.status}{receipt.error_code ? ` · ${receipt.error_code}` : ""}
                  </span>
                  <span>{receipt.latency_ms} ms</span>
                  <span>{formatTokens(
                    receipt,
                    copy("Tokens unknown", "token 未知", "token 未知", "token は未知"),
                    (total) => copy(
                      `${total} reported tokens`,
                      `${total} 上游上报 Token`,
                      `${total} 上游上報 Token`,
                      `${total} 報告トークン`,
                    ),
                  )}</span>
                  <span className={`receipt-cost ${receipt.cost_kind}`}>
                    {formatCost(
                      receipt,
                      copy("Actual cost", "实际成本", "實際成本", "実際のコスト"),
                      copy("Estimated cost", "估算成本", "估算成本", "推定コスト"),
                      copy("Cost unknown", "成本未知", "費用未知", "費用は不明"),
                    )}
                  </span>
                </summary>
                <div className="receipt-meta">
                  <code title={receipt.request_id}>{receipt.request_id}</code>
                  <CopyRequestId requestId={receipt.request_id} />
                  <span>{receipt.stream ? copy("Streaming", "流式", "流式", "ストリーム") : copy("Non-streaming", "非流式", "非流式", "非ストリーム")}</span>
                  <span>{copy(`Requested model ${receipt.requested_model}`, `请求模型 ${receipt.requested_model}`, `請求模型 ${receipt.requested_model}`, `モデルを要求 ${receipt.requested_model}`)}</span>
                  <span>{copy(`${receipt.attempts} attempts`, `${receipt.attempts} 次尝试`, `${receipt.attempts} 次嘗試`, `${receipt.attempts} 回の試行`)}</span>
                  {receipt.running_revision != null && <span>revision {receipt.running_revision}</span>}
                  {receipt.price_version != null && receipt.cost_kind !== "unknown" && <span>price v{receipt.price_version}</span>}
                </div>
                <ReceiptDetails receipt={receipt} />
              </details>
            );
          })}
        </div>
      )}
    </section>
  );
}
