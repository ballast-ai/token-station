import { useCallback, useEffect, useRef, useState } from "react";
import {
  getRecentReceipts,
  type ReceiptDecidedByView,
  type ReceiptFeaturesView,
  type ReceiptRouteView,
  type ReceiptView,
} from "../api";
import { humanizeAppError, humanizeReceiptError } from "../errors";
import { useLocalizedCopy } from "./LanguageProvider";

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

function formatTokens(receipt: ReceiptView, unknown: string): string {
  if (!receipt.usage) return unknown;
  return `${receipt.usage.input_tokens + receipt.usage.output_tokens} tokens`;
}

function formatDecisionReason(
  reason: ReceiptDecidedByView,
  copy: (english: string, simplifiedChinese: string) => string,
): string {
  switch (reason.tier) {
    case "rule":
      return copy(`Rule · ${reason.rule}`, `规则 · ${reason.rule}`);
    case "hint":
      return copy(`Hint · ${reason.kind}/${reason.value}`, `提示 · ${reason.kind}/${reason.value}`);
    case "heuristic":
      return copy(
        `Heuristic score ${reason.score} · matched band ≥ ${reason.matched_band_at_least}`,
        `启发式评分 ${reason.score} · 命中档位下界 ≥ ${reason.matched_band_at_least}`,
      );
    case "exact_model":
      return copy(`Exact model · ${reason.model}`, `指定模型 · ${reason.model}`);
    case "quota":
      return copy("Quota-first", "额度优先");
    default:
      return copy("Default route", "默认路由");
  }
}

/** One-line quota decision summary: remaining quota, time to reset, rate headroom, and state. */
function formatQuotaDecision(
  quota: NonNullable<ReceiptRouteView["quota"]>,
  copy: (english: string, simplifiedChinese: string) => string,
): string {
  const parts: string[] = [];
  if (quota.remaining_permille != null) {
    parts.push(copy(
      `${(quota.remaining_permille / 10).toFixed(0)}% left`,
      `剩 ${(quota.remaining_permille / 10).toFixed(0)}%`,
    ));
  }
  const NO_RESET_MS = 100 * 24 * 60 * 60 * 1000;
  if (quota.reset_ms != null && quota.reset_ms > 0 && quota.reset_ms < NO_RESET_MS) {
    const minutes = Math.round(quota.reset_ms / 60000);
    parts.push(copy(`resets in ${minutes}m`, `${minutes}分钟后刷新`));
  }
  parts.push(copy(
    `headroom ${(quota.headroom_permille / 10).toFixed(0)}%`,
    `速率余量 ${(quota.headroom_permille / 10).toFixed(0)}%`,
  ));
  if (quota.exhausted) parts.push(copy("exhausted", "已耗尽"));
  else if (quota.pressured) parts.push(copy("pressured", "速率吃紧"));
  return parts.join(" · ");
}

function formatFeatures(
  features: ReceiptFeaturesView,
  copy: (english: string, simplifiedChinese: string) => string,
): string {
  const flags = [
    features.tool_count ? `${features.tool_count} tools` : null,
    features.has_images ? copy("images", "图像") : null,
    features.requires_json_schema ? "JSON Schema" : null,
    features.code_block_count ? `${features.code_block_count} code blocks` : null,
  ].filter(Boolean);
  return copy(
    `${features.estimated_input_tokens} estimated input tokens · ${features.message_count} messages${flags.length ? ` · ${flags.join(" · ")}` : ""}`,
    `${features.estimated_input_tokens} 估算输入 token · ${features.message_count} 条消息${flags.length ? ` · ${flags.join(" · ")}` : ""}`,
  );
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
    <div className="receipt-timeline">
      {diagnosis && (
        <section className="receipt-diagnosis" aria-label={copy("Error diagnosis", "错误诊断")}>
          <h4>Diagnosis</h4>
          <strong>{diagnosis.layer}</strong>
          <span>{diagnosis.message}</span>
          <small>{copy(`Next: ${diagnosis.suggestion}`, `下一步：${diagnosis.suggestion}`)}</small>
        </section>
      )}
      <section aria-label={copy("Decision record", "决策记录")}>
        <h4>Decision</h4>
        {receipt.decision ? (
          <div className="receipt-event">
            <span className="receipt-event-index">D</span>
            <div>
              <strong>{formatRoute(receipt.decision, copy("No route", "未产生路由"))}</strong>
              <span>
                {receipt.decision.pool} · {formatDecisionReason(receipt.decision.decided_by, copy)} · {copy(
                  `${receipt.decision.fallbacks} fallback candidates`,
                  `${receipt.decision.fallbacks} 个候选回退`,
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
              "请求在本地入站转换阶段停止，尚未进入路由。",
            )
            : copy("No decision record", "没有决策记录")}</p>
        )}
      </section>

      <section aria-label={copy("Upstream attempt records", "上游尝试记录")}>
        <h4>Attempts</h4>
        {attempts.length ? attempts.map((attempt) => (
          <div className="receipt-event" key={attempt.ordinal}>
            <span className="receipt-event-index">{attempt.ordinal}</span>
            <div>
              <strong>{attempt.upstream}/{attempt.model}</strong>
              <span>
                {attempt.http_status
                  ? `HTTP ${attempt.http_status}`
                  : attempt.error_code ?? copy("No final HTTP status", "无 HTTP 终态")} · {attempt.latency_ms} ms
              </span>
              <small>
                {attempt.stream_outcome ?? copy("Non-streaming", "非流式")} · {attempt.fallback_allowed
                  ? copy("Fallback allowed", "允许回退")
                  : copy("Fallback blocked", "不允许回退")}
              </small>
            </div>
          </div>
        )) : <p className="receipt-section-empty">{stoppedDuringInbound
          ? copy(
            "The request was not sent upstream because local conversion failed.",
            "本地转换失败，请求未发往上游。",
          )
          : copy("No upstream attempts", "没有真实上游尝试")}</p>}
      </section>

      <section aria-label={copy("Protocol conversion records", "协议转换记录")}>
        <h4>Conversions</h4>
        {conversions.length ? conversions.map((conversion) => (
          <div className="receipt-event" key={conversion.ordinal}>
            <span className="receipt-event-index">{conversion.ordinal}</span>
            <div>
              <strong>{conversion.stage}</strong>
              <span>{conversion.source_protocol} → {conversion.target_protocol}</span>
              <small>{conversion.outcome === "cancelled"
                ? copy("Cancelled by client", "客户端已取消")
                : conversion.succeeded
                  ? copy("Succeeded", "成功")
                  : [conversion.error_code, conversion.reason_code, conversion.reason_detail]
                    .filter(Boolean)
                    .join(" · ") || copy("Failed", "失败")}</small>
            </div>
          </div>
        )) : <p className="receipt-section-empty">{copy("No conversion records", "没有转换记录")}</p>}
      </section>
    </div>
  );
}

function CopyRequestId({ requestId }: { requestId: string }) {
  const { copy } = useLocalizedCopy();
  const [copied, setCopied] = useState(false);
  const copyRequestId = async () => {
    await navigator.clipboard?.writeText(requestId);
    setCopied(true);
  };
  return (
    <button type="button" className="btn ghost tiny receipt-copy" onClick={() => void copyRequestId()}>
      {copied ? copy("Copied", "已复制") : copy("Copy request ID", "复制请求 ID")}
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
      if (mounted.current) setError(humanizeAppError(caught));
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
  }, []);
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
          <span className="eyebrow">REQUEST RECEIPTS</span>
          <h2 id="recent-receipts-heading">{copy("Recent requests", "最近 5 次请求")}</h2>
          <p className="sub">{copy(
            "Only route, final status, and usage metadata are stored. Request and response bodies are never recorded.",
            "仅保留路由、终态与用量元数据，不记录请求或响应正文。",
          )}</p>
        </div>
        <div className="receipt-refresh-actions">
          <span className="receipt-refresh-note">
            {copy("Refreshes every 10 seconds", "每 10 秒自动刷新")}
            {lastUpdatedAt != null && (
              <>
                {" · "}
                <time
                  data-testid="receipt-updated-at"
                  dateTime={new Date(lastUpdatedAt).toISOString()}
                >
                  {copy(
                    `${formatUpdatedAt(lastUpdatedAt, language)} updated`,
                    `${formatUpdatedAt(lastUpdatedAt, language)} 更新`,
                  )}
                </time>
              </>
            )}
          </span>
          {receipts && receipts.length > 0 && (
            <span className="count-badge">{copy(
              `${receipts.length} records`,
              `${receipts.length} 条`,
            )}</span>
          )}
          <button
            type="button"
            className={`btn receipt-refresh-button${busy ? " busy" : ""}`}
            aria-label={copy("Refresh recent requests", "刷新最近请求")}
            aria-busy={busy}
            disabled={busy}
            onClick={() => void loadReceipts(true)}
          >
            <RefreshIcon />
            <span>{busy
              ? copy("Refreshing…", "刷新中…")
              : copy("Refresh", "刷新")}</span>
          </button>
        </div>
      </div>

      {!receipts && !error && (
        <div className="receipt-state" role="status">{copy(
          "Loading request receipts…",
          "正在读取请求回执…",
        )}</div>
      )}
      {error && receipts === null && (
        <div className="receipt-state error-text" role="alert">
          {copy(`Failed to load request receipts: ${error}`, `请求回执读取失败：${error}`)}
        </div>
      )}
      {error && receipts !== null && (
        <p className="receipt-refresh-error error-text" role="status">
          {copy(
            `Refresh failed; showing the last successful data: ${error}`,
            `更新失败，当前显示上次数据：${error}`,
          )}
        </p>
      )}
      {receipts?.length === 0 && (
        <div className="empty-state">
          <strong>{copy("No request receipts yet", "还没有请求回执")}</strong>
          <span>{copy(
            "Start the proxy and complete a request to create a body-free receipt.",
            "启动代理并完成一次请求后，这里会显示无正文回执。",
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
                    {receipt.agent_id ?? copy("Unknown Agent", "未知 Agent")} · {receipt.protocol}
                  </span>
                  <strong className="receipt-route">
                    {formatRoute(receipt.routing, copy("No route", "未产生路由"))}
                  </strong>
                  <span className={`receipt-status ${ok ? "success" : "failure"}`}>
                    HTTP {receipt.status}{receipt.error_code ? ` · ${receipt.error_code}` : ""}
                  </span>
                  <span>{receipt.latency_ms} ms</span>
                  <span>{formatTokens(receipt, copy("Tokens unknown", "token 未知"))}</span>
                  <span className={`receipt-cost ${receipt.cost_kind}`}>
                    {formatCost(
                      receipt,
                      copy("Actual cost", "实际成本"),
                      copy("Estimated cost", "估算成本"),
                      copy("Cost unknown", "成本未知"),
                    )}
                  </span>
                </summary>
                <div className="receipt-meta">
                  <code title={receipt.request_id}>{receipt.request_id}</code>
                  <CopyRequestId requestId={receipt.request_id} />
                  <span>{receipt.stream ? copy("Streaming", "流式") : copy("Non-streaming", "非流式")}</span>
                  <span>{copy(`Requested model ${receipt.requested_model}`, `请求模型 ${receipt.requested_model}`)}</span>
                  <span>{copy(`${receipt.attempts} attempts`, `${receipt.attempts} 次尝试`)}</span>
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
