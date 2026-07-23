import { useEffect, useState } from "react";
import {
  getRecentReceipts,
  type ReceiptDecidedByView,
  type ReceiptFeaturesView,
  type ReceiptRouteView,
  type ReceiptView,
} from "../api";
import { humanizeErrorCode } from "../errors";

const MAX_RECEIPTS = 5;

function formatTime(timestamp: number): string {
  return new Date(timestamp).toLocaleString("zh-CN", {
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    hour12: false,
  });
}

function formatRoute(route: ReceiptRouteView | null): string {
  return route ? `${route.upstream}/${route.model}` : "未产生路由";
}

function formatCost(receipt: ReceiptView): string {
  if (receipt.cost_kind === "unknown" || receipt.cost_micros == null) return "成本未知";
  const amount = (receipt.cost_micros / 1_000_000).toFixed(6);
  return receipt.cost_kind === "actual" ? `实际成本 ${amount}` : `估算成本 ${amount}`;
}

function formatTokens(receipt: ReceiptView): string {
  if (!receipt.usage) return "token 未知";
  return `${receipt.usage.input_tokens + receipt.usage.output_tokens} tokens`;
}

function formatDecisionReason(reason: ReceiptDecidedByView): string {
  switch (reason.tier) {
    case "rule":
      return `规则 · ${reason.rule}`;
    case "hint":
      return `提示 · ${reason.kind}/${reason.value}`;
    case "heuristic":
      return `启发式 · ${reason.score}/${reason.threshold}`;
    case "exact_model":
      return `指定模型 · ${reason.model}`;
    default:
      return "默认路由";
  }
}

function formatFeatures(features: ReceiptFeaturesView): string {
  const flags = [
    features.tool_count ? `${features.tool_count} tools` : null,
    features.has_images ? "图像" : null,
    features.requires_json_schema ? "JSON Schema" : null,
    features.code_block_count ? `${features.code_block_count} code blocks` : null,
  ].filter(Boolean);
  return `${features.estimated_input_tokens} 估算输入 token · ${features.message_count} 条消息${
    flags.length ? ` · ${flags.join(" · ")}` : ""
  }`;
}

export function ReceiptDetails({ receipt }: { receipt: ReceiptView }) {
  const attempts = receipt.attempt_records ?? [];
  const conversions = receipt.conversion_reports ?? [];
  const diagnosis = humanizeErrorCode(receipt.error_code);
  return (
    <div className="receipt-timeline">
      {diagnosis && (
        <section className="receipt-diagnosis" aria-label="错误诊断">
          <h4>Diagnosis</h4>
          <strong>{diagnosis.layer}</strong>
          <span>{diagnosis.message}</span>
          <small>下一步：{diagnosis.suggestion}</small>
        </section>
      )}
      <section aria-label="决策记录">
        <h4>Decision</h4>
        {receipt.decision ? (
          <div className="receipt-event">
            <span className="receipt-event-index">D</span>
            <div>
              <strong>{formatRoute(receipt.decision)}</strong>
              <span>
                {receipt.decision.pool} · {formatDecisionReason(receipt.decision.decided_by)} · {receipt.decision.fallbacks} 个候选回退
              </span>
              <small>{formatFeatures(receipt.decision.features)}</small>
            </div>
          </div>
        ) : (
          <p className="receipt-section-empty">没有决策记录</p>
        )}
      </section>

      <section aria-label="上游尝试记录">
        <h4>Attempts</h4>
        {attempts.length ? attempts.map((attempt) => (
          <div className="receipt-event" key={attempt.ordinal}>
            <span className="receipt-event-index">{attempt.ordinal}</span>
            <div>
              <strong>{attempt.upstream}/{attempt.model}</strong>
              <span>
                {attempt.http_status ? `HTTP ${attempt.http_status}` : attempt.error_code ?? "无 HTTP 终态"} · {attempt.latency_ms} ms
              </span>
              <small>
                {attempt.stream_outcome ?? "非流式"} · {attempt.fallback_allowed ? "允许回退" : "不允许回退"}
              </small>
            </div>
          </div>
        )) : <p className="receipt-section-empty">没有真实上游尝试</p>}
      </section>

      <section aria-label="协议转换记录">
        <h4>Conversions</h4>
        {conversions.length ? conversions.map((conversion) => (
          <div className="receipt-event" key={conversion.ordinal}>
            <span className="receipt-event-index">{conversion.ordinal}</span>
            <div>
              <strong>{conversion.stage}</strong>
              <span>{conversion.source_protocol} → {conversion.target_protocol}</span>
              <small>{conversion.succeeded ? "成功" : conversion.error_code ?? "失败"}</small>
            </div>
          </div>
        )) : <p className="receipt-section-empty">没有转换记录</p>}
      </section>
    </div>
  );
}

function CopyRequestId({ requestId }: { requestId: string }) {
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    await navigator.clipboard?.writeText(requestId);
    setCopied(true);
  };
  return (
    <button type="button" className="btn ghost tiny receipt-copy" onClick={() => void copy()}>
      {copied ? "已复制" : "复制请求 ID"}
    </button>
  );
}

export default function RecentReceipts() {
  const [receipts, setReceipts] = useState<ReceiptView[] | null>(null);
  const [error, setError] = useState("");

  useEffect(() => {
    let active = true;
    getRecentReceipts(MAX_RECEIPTS)
      .then((result) => {
        if (active) setReceipts(result.slice(0, MAX_RECEIPTS));
      })
      .catch((caught) => {
        if (active) setError(String(caught));
      });
    return () => {
      active = false;
    };
  }, []);

  return (
    <section className="panel receipt-panel" aria-labelledby="recent-receipts-heading">
      <div className="panel-head split-heading">
        <div>
          <span className="eyebrow">REQUEST RECEIPTS</span>
          <h2 id="recent-receipts-heading">最近 5 次请求</h2>
          <p className="sub">仅保留路由、终态与用量元数据，不记录请求或响应正文。</p>
        </div>
        {receipts && receipts.length > 0 && <span className="count-badge">{receipts.length} 条</span>}
      </div>

      {!receipts && !error && <div className="receipt-state" role="status">正在读取请求回执…</div>}
      {error && <div className="receipt-state error-text" role="alert">请求回执读取失败：{error}</div>}
      {receipts?.length === 0 && (
        <div className="empty-state">
          <strong>还没有请求回执</strong>
          <span>启动代理并完成一次请求后，这里会显示无正文回执。</span>
        </div>
      )}

      {receipts && receipts.length > 0 && (
        <div className="receipt-list">
          {receipts.map((receipt) => {
            const ok = receipt.status >= 200 && receipt.status < 400 && !receipt.error_code;
            return (
              <details className="receipt-row" data-testid="receipt-row" key={receipt.request_id}>
                <summary>
                  <time dateTime={new Date(receipt.started_at_ms).toISOString()}>{formatTime(receipt.started_at_ms)}</time>
                  <span className="receipt-agent">{receipt.agent_id ?? "未知 Agent"} · {receipt.protocol}</span>
                  <strong className="receipt-route">{formatRoute(receipt.routing)}</strong>
                  <span className={`receipt-status ${ok ? "success" : "failure"}`}>
                    HTTP {receipt.status}{receipt.error_code ? ` · ${receipt.error_code}` : ""}
                  </span>
                  <span>{receipt.latency_ms} ms</span>
                  <span>{formatTokens(receipt)}</span>
                  <span className={`receipt-cost ${receipt.cost_kind}`}>{formatCost(receipt)}</span>
                </summary>
                <div className="receipt-meta">
                  <code title={receipt.request_id}>{receipt.request_id}</code>
                  <CopyRequestId requestId={receipt.request_id} />
                  <span>{receipt.stream ? "流式" : "非流式"}</span>
                  <span>请求模型 {receipt.requested_model}</span>
                  <span>{receipt.attempts} 次尝试</span>
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
