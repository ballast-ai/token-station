import { useEffect, useState } from "react";
import { getReceipts, type Receipt } from "../api";
import { humanizeErrorCode } from "../errors";

function formatCost(micros: number | null): string {
  if (micros == null) return "成本未知";
  return `$${(micros / 1_000_000).toFixed(4)}`;
}

function statusTone(status: number): string {
  if (status >= 200 && status < 300) return "ok";
  if (status === 499) return "cancel";
  return "err";
}

/** Recent request receipts: one per row. Open a row to see the routing reason, attempts, and terminal state. Metadata only; no content. */
export default function RecentRequests() {
  const [receipts, setReceipts] = useState<Receipt[]>([]);
  const [error, setError] = useState("");
  const [open, setOpen] = useState<string | null>(null);
  const [copied, setCopied] = useState<string | null>(null);

  const refresh = () => {
    getReceipts(5)
      .then((rows) => {
        setReceipts(rows);
        setError("");
      })
      .catch((caught) => setError(String(caught)));
  };
  useEffect(refresh, []);

  return (
    <section className="panel receipts-panel" aria-label="最近请求回执">
      <div className="panel-head split-heading">
        <h2>最近请求</h2>
        <button type="button" className="btn ghost" onClick={refresh}>
          刷新
        </button>
      </div>
      {error ? (
        <div className="banner err">
          {error}
          {receipts.length > 0 ? "（下方为上次成功获取的数据）" : ""}
        </div>
      ) : null}
      {receipts.length === 0 && !error ? (
        <p className="empty-note">
          还没有请求记录。代理跑起来、有流量后，这里显示最近 5 次的路由回执——为什么走这个模型、fallback 了谁、花了多少。
        </p>
      ) : (
        <ul className="receipt-list">
          {receipts.map((r) => {
            const id = r.request_id || String(r.started_at_ms);
            return (
              <li key={id} className="receipt-row">
                <button
                  type="button"
                  className="receipt-summary"
                  onClick={() => setOpen(open === id ? null : id)}
                >
                  <span className={`receipt-dot ${statusTone(r.status)}`} />
                  <span className="mono">{r.requested_model}</span>
                  <span className="muted">→</span>
                  <span className="mono">{r.model ?? "—"}</span>
                  {r.tier ? <span className="chip">{r.tier}</span> : null}
                  <span className="receipt-spacer" />
                  <span className="muted">{r.latency_ms}ms</span>
                  <span className="muted">{formatCost(r.cost_micros)}</span>
                </button>
                {open === id ? (
                  <dl className="receipt-detail">
                    <div>
                      <dt>请求 ID</dt>
                      <dd className="mono req-id-cell">
                        {r.request_id || "—"}
                        {r.request_id ? (
                          <button
                            type="button"
                            className="btn ghost tiny"
                            title="复制请求 ID"
                            onClick={() => {
                              void navigator.clipboard?.writeText(r.request_id);
                              setCopied(r.request_id);
                            }}
                          >
                            {copied === r.request_id ? "已复制" : "复制"}
                          </button>
                        ) : null}
                      </dd>
                    </div>
                    <div>
                      <dt>实际 Provider</dt>
                      <dd className="mono">{r.upstream ?? "—"}</dd>
                    </div>
                    <div>
                      <dt>档位 / 理由</dt>
                      <dd>
                        {r.pool ?? "—"} · {r.tier ?? "—"}
                      </dd>
                    </div>
                    <div>
                      <dt>尝试次数</dt>
                      <dd>{r.attempts}</dd>
                    </div>
                    <div>
                      <dt>终态</dt>
                      <dd>
                        {r.status}
                        {r.error_code ? ` · ${r.error_code}` : ""}
                      </dd>
                    </div>
                    {(() => {
                      const human = humanizeErrorCode(r.error_code);
                      return human ? (
                        <div className="receipt-diagnosis">
                          <dt>诊断</dt>
                          <dd>
                            <span className="diag-layer">{human.layer}</span>
                            <p className="diag-message">{human.message}</p>
                            <p className="diag-suggestion">→ {human.suggestion}</p>
                          </dd>
                        </div>
                      ) : null;
                    })()}
                  </dl>
                ) : null}
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}
