import { useEffect, useMemo, useState } from "react";
import type { AgentConfirmationKind, ConfigPlanView } from "../api";

const CONFIRMATION_COPY: Record<AgentConfirmationKind, string> = {
  installation: "我确认这是要操作的安装实例",
  target_config: "我确认目标配置文件路径",
  configuration_diff: "我已检查字段级变更",
  experimental_compatibility: "我理解该版本属于推定兼容，并接受额外风险",
};

interface AgentChangePreviewProps {
  plan: ConfigPlanView;
  busy: boolean;
  error: string;
  onCancel: () => void;
  onApply: () => void;
}
export default function AgentChangePreview({
  plan,
  busy,
  error,
  onCancel,
  onApply,
}: AgentChangePreviewProps) {
  const [confirmed, setConfirmed] = useState<Set<AgentConfirmationKind>>(new Set());
  const [now, setNow] = useState(Date.now());

  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, []);

  useEffect(() => setConfirmed(new Set()), [plan.operation_id]);

  const expired = now >= plan.expires_at_ms;
  const allConfirmed = plan.required_confirmations.every((item) => confirmed.has(item));
  const title =
    plan.intent === "connect" ? "确认接入计划" : plan.intent === "disconnect" ? "确认断开计划" : "确认恢复计划";
  const action = plan.intent === "connect" ? "确认并接入" : plan.intent === "disconnect" ? "确认并断开" : "确认并恢复";
  const seconds = useMemo(
    () => Math.max(0, Math.ceil((plan.expires_at_ms - now) / 1000)),
    [now, plan.expires_at_ms],
  );

  return (
    <div className="agent-dialog-backdrop" role="presentation">
      <section className="agent-dialog" role="dialog" aria-modal="true" aria-labelledby="agent-preview-title">
        <header className="agent-dialog-head">
          <div>
            <span className="dialog-eyebrow">SERVER-BOUND PLAN</span>
            <h2 id="agent-preview-title">{title}</h2>
          </div>
          <span className={`plan-clock ${expired ? "expired" : ""}`}>
            {expired ? "计划已过期" : `${seconds}s 后过期`}
          </span>
        </header>

        {plan.required_confirmations.includes("experimental_compatibility") && (
          <div className="experimental-warning">
            该版本未经过完整验证。只读结构预检已通过，但仍需额外确认。
          </div>
        )}

        <div className="plan-bindings">
          <div><span>Agent</span><strong>{plan.agent_id}</strong></div>
          <div><span>安装</span><code>{plan.installation_path}</code></div>
          <div><span>目标</span><code>{plan.target_config_path}</code></div>
          <div><span>兼容证据</span><strong>{plan.compatibility_evidence.status}</strong></div>
        </div>

        <div className="plan-diff">
          <div className="plan-section-title">字段级差异 · 敏感值已隐藏</div>
          <pre>{plan.human_diff || "没有字段变化"}</pre>
        </div>

        <div className="plan-confirmations">
          {plan.required_confirmations.map((item) => (
            <label key={item}>
              <input
                type="checkbox"
                checked={confirmed.has(item)}
                disabled={busy || expired}
                onChange={(event) => {
                  setConfirmed((current) => {
                    const next = new Set(current);
                    if (event.target.checked) next.add(item);
                    else next.delete(item);
                    return next;
                  });
                }}
              />
              <span>{CONFIRMATION_COPY[item]}</span>
            </label>
          ))}
        </div>

        {expired && <div className="dialog-error">计划已失效。关闭后必须重新预览，旧 operation 不能继续使用。</div>}
        {error && <div className="dialog-error">{error}</div>}

        <footer className="agent-dialog-actions">
          <button className="btn" type="button" disabled={busy} onClick={onCancel}>取消</button>
          <button
            className="btn primary"
            type="button"
            disabled={busy || expired || !allConfirmed}
            onClick={onApply}
          >
            {busy ? "正在执行…" : action}
          </button>
        </footer>
      </section>
    </div>
  );
}
