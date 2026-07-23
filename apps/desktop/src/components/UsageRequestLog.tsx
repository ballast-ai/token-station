import { useEffect, useMemo, useState } from "react";
import {
  getRequestReceipts,
  type ReceiptCostKind,
  type ReceiptPageView,
  type ReceiptView,
} from "../api";
import { ReceiptDetails } from "./RecentReceipts";

const PAGE_SIZE = 20;

interface UsageRequestLogProps {
  since: string;
  agentId: string;
  upstream: string;
  model: string;
  refreshKey: number;
}

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

function routeOf(receipt: ReceiptView): string {
  return receipt.routing
    ? `${receipt.routing.upstream}/${receipt.routing.model}`
    : "未产生路由";
}

function tokenTotal(receipt: ReceiptView): string {
  if (!receipt.usage) return "—";
  return (receipt.usage.input_tokens + receipt.usage.output_tokens).toLocaleString();
}

function costLabel(receipt: ReceiptView): string {
  if (receipt.cost_kind === "unknown" || receipt.cost_micros == null) return "成本未知";
  const prefix = receipt.cost_kind === "actual" ? "实际" : "估算";
  return `${prefix} $${(receipt.cost_micros / 1_000_000).toFixed(6)}`;
}

function unknownCostReason(receipt: ReceiptView): string {
  if (receipt.usage == null) return "上游未返回 Token，无法估算成本";
  const model = receipt.routing?.model || receipt.requested_model;
  return `缺少模型价格：${model}`;
}

function CostState({
  receipt,
  compact = false,
}: {
  receipt: ReceiptView;
  compact?: boolean;
}) {
  const kind: ReceiptCostKind = receipt.cost_kind;
  if (kind === "unknown" || receipt.cost_micros == null) {
    return (
      <span className="usage-log-cost unknown" title={unknownCostReason(receipt)}>
        {compact ? "未知" : unknownCostReason(receipt)}
      </span>
    );
  }
  return <span className={`usage-log-cost ${kind}`}>{costLabel(receipt)}</span>;
}

export default function UsageRequestLog({
  since,
  agentId,
  upstream,
  model,
  refreshKey,
}: UsageRequestLogProps) {
  const [status, setStatus] = useState<"" | "success" | "error">("");
  const [page, setPage] = useState(1);
  const [data, setData] = useState<ReceiptPageView | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");

  useEffect(() => {
    setPage(1);
  }, [since, agentId, upstream, model, status]);

  useEffect(() => {
    let active = true;
    setLoading(true);
    setError("");
    getRequestReceipts({
      since,
      agentId: agentId || null,
      upstream: upstream || null,
      model: model || null,
      status: status || null,
      page,
      pageSize: PAGE_SIZE,
    })
      .then((next) => {
        if (active) setData(next);
      })
      .catch((caught) => {
        if (active) setError(String(caught));
      })
      .finally(() => {
        if (active) setLoading(false);
      });
    return () => {
      active = false;
    };
  }, [agentId, model, page, refreshKey, since, status, upstream]);

  const total = data?.total ?? 0;
  const totalPages = Math.max(1, Math.ceil(total / PAGE_SIZE));
  const range = useMemo(() => {
    if (!total) return "0 条";
    const start = (page - 1) * PAGE_SIZE + 1;
    return `${start}–${Math.min(total, start + PAGE_SIZE - 1)} / ${total}`;
  }, [page, total]);

  return (
    <section className="usage-request-log">
      <header>
        <div>
          <h2>请求日志</h2>
          <p>完整本地 Receipt · 不含请求或响应正文</p>
        </div>
        <div className="usage-log-tools">
          <label>
            <span>状态</span>
            <select
              aria-label="请求状态"
              value={status}
              onChange={(event) => setStatus(event.target.value as typeof status)}
            >
              <option value="">全部状态</option>
              <option value="success">成功</option>
              <option value="error">失败</option>
            </select>
          </label>
          <strong>{range}</strong>
        </div>
      </header>

      {loading && !data && <div className="usage-log-state">正在读取请求日志…</div>}
      {error && <div className="usage-log-state error-text">请求日志读取失败：{error}</div>}
      {!loading && !error && data?.items.length === 0 && (
        <div className="usage-log-state">当前筛选范围没有请求日志。</div>
      )}

      {data && data.items.length > 0 && (
        <>
          <div className="usage-log-head" aria-hidden="true">
            <span>时间</span><span>Agent / 路由</span><span>状态</span>
            <span>Token</span><span>延迟</span><span>成本</span>
          </div>
          <div className={`usage-log-list ${loading ? "refreshing" : ""}`}>
            {data.items.map((receipt) => {
              const success = receipt.status >= 200
                && receipt.status < 400
                && receipt.error_code == null;
              return (
                <details className="usage-log-row" key={receipt.request_id}>
                  <summary>
                    <time dateTime={new Date(receipt.started_at_ms).toISOString()}>
                      {formatTime(receipt.started_at_ms)}
                    </time>
                    <span className="usage-log-route">
                      <small>{receipt.agent_id ?? "未知 Agent"}</small>
                      <strong>{routeOf(receipt)}</strong>
                    </span>
                    <span className={`usage-log-status ${success ? "success" : "error"}`}>
                      HTTP {receipt.status}
                    </span>
                    <span>{tokenTotal(receipt)}</span>
                    <span>{receipt.latency_ms.toLocaleString()} ms</span>
                    <CostState receipt={receipt} compact />
                  </summary>
                  <div className="usage-log-expanded">
                    <div className="usage-log-facts">
                      <span><small>请求 ID</small><code>{receipt.request_id}</code></span>
                      <span><small>请求模型</small><strong>{receipt.requested_model}</strong></span>
                      <span><small>协议</small><strong>{receipt.protocol}</strong></span>
                      <span><small>传输</small><strong>{receipt.stream ? "流式" : "非流式"}</strong></span>
                      {receipt.price_version != null && (
                        <span><small>价格版本</small><strong>v{receipt.price_version}</strong></span>
                      )}
                    </div>
                    {receipt.usage && (
                      <div className="usage-log-token-facts">
                        <span>输入 <strong>{receipt.usage.input_tokens.toLocaleString()}</strong></span>
                        <span>输出 <strong>{receipt.usage.output_tokens.toLocaleString()}</strong></span>
                        <span>缓存读 <strong>{receipt.usage.cache_read_tokens.toLocaleString()}</strong></span>
                        <span>缓存写 <strong>{receipt.usage.cache_write_tokens.toLocaleString()}</strong></span>
                        <span>推理 <strong>{receipt.usage.reasoning_tokens.toLocaleString()}</strong></span>
                      </div>
                    )}
                    <CostState receipt={receipt} />
                    <ReceiptDetails receipt={receipt} />
                  </div>
                </details>
              );
            })}
          </div>
          <footer className="usage-log-pagination">
            <button
              type="button"
              disabled={page <= 1 || loading}
              onClick={() => setPage((value) => Math.max(1, value - 1))}
            >
              上一页
            </button>
            <span>第 {page} / {totalPages} 页</span>
            <button
              type="button"
              aria-label="下一页"
              disabled={page >= totalPages || loading}
              onClick={() => setPage((value) => Math.min(totalPages, value + 1))}
            >
              下一页
            </button>
          </footer>
        </>
      )}
    </section>
  );
}
