import type { AgentDriftView, DriftStatus } from "../api";

interface AgentDriftPanelProps {
  views: AgentDriftView[] | null;
  loading: boolean;
  error: string;
}

const statusLabels: Record<DriftStatus, string> = {
  unmanaged: "尚未接管",
  in_sync: "无漂移",
  unowned_changes: "检测到非受管改动",
  managed_changes: "受管字段已被外部修改",
  missing: "配置文件缺失",
  unreadable: "配置事实不可读",
  unparseable: "当前配置无法解析",
};

const kindLabels = {
  added: "新增",
  removed: "删除",
  changed: "修改",
} as const;

function shortHash(hash: string | null) {
  return hash ? hash.slice(0, 12) : "—";
}

function pathLabel(segments: string[]) {
  return `/${segments.join("/")}`;
}

export default function AgentDriftPanel({ views, loading, error }: AgentDriftPanelProps) {
  return (
    <section className="agent-drift-panel panel" aria-label="配置漂移对账">
      <div className="agent-drift-head">
        <div>
          <span className="eyebrow">CONFIG DRIFT</span>
          <strong>配置漂移对账</strong>
        </div>
        <span>只读 · 不自动覆盖</span>
      </div>

      {loading && <div className="agent-drift-empty">正在核对接管前、最后写入与当前磁盘…</div>}
      {!loading && error && <div className="manager-error">配置对账失败：{error}</div>}
      {!loading && !error && views?.length === 0 && (
        <div className="agent-drift-empty">该精确安装尚无 Token Station 接管记录。</div>
      )}

      {!loading && !error && views?.map((view) => (
        <article className={`agent-drift-record ${view.status}`} key={view.target_config_path}>
          <div className="agent-drift-record-head">
            <code>{view.target_config_path}</code>
            <span>{statusLabels[view.status]}</span>
          </div>
          <div className="agent-drift-versions">
            <div><span>接管前</span><code title={view.baseline_hash}>{shortHash(view.baseline_hash)}</code></div>
            <div><span>最后写入</span><code title={view.managed_hash}>{shortHash(view.managed_hash)}</code></div>
            <div><span>当前磁盘</span><code title={view.current_hash ?? ""}>{shortHash(view.current_hash)}</code></div>
          </div>
          <p>{view.message}</p>
          {view.changes.length > 0 && (
            <div className="agent-drift-changes">
              {view.changes.map((change, index) => (
                <div key={`${pathLabel(change.path.segments)}-${index}`}>
                  <code>{pathLabel(change.path.segments)}</code>
                  <span>{change.scope === "managed" ? "受管字段" : "非受管字段"}</span>
                  <span>{kindLabels[change.kind]}</span>
                </div>
              ))}
              {view.truncated && <small>变化过多，仅显示前 200 条。</small>}
            </div>
          )}
        </article>
      ))}
    </section>
  );
}
