import type { SnapshotView } from "../api";

interface AgentSnapshotListProps {
  agentName: string;
  snapshots: SnapshotView[];
  loading: boolean;
  error: string;
  busy: boolean;
  onRestore: (snapshot: SnapshotView) => void;
  onClose: () => void;
}

export default function AgentSnapshotList({
  agentName,
  snapshots,
  loading,
  error,
  busy,
  onRestore,
  onClose,
}: AgentSnapshotListProps) {
  return (
    <section className="snapshot-panel" aria-label={`${agentName} 快照`}>
      <header>
        <div>
          <span className="dialog-eyebrow">ENCRYPTED HISTORY</span>
          <h2>{agentName} 快照</h2>
        </div>
        <button className="btn quiet" type="button" onClick={onClose}>关闭</button>
      </header>
      {loading && <div className="agent-page-empty">正在读取快照索引…</div>}
      {error && <div className="dialog-error">{error}</div>}
      {!loading && !error && snapshots.length === 0 && (
        <div className="agent-page-empty">尚无快照。首次确认接入前会自动创建加密基线。</div>
      )}
      <div className="snapshot-list">
        {snapshots.map((snapshot) => (
          <article key={snapshot.snapshot_id}>
            <div>
              <strong>{new Date(snapshot.created_at_ms).toLocaleString()}</strong>
              <code>{snapshot.target_config_path}</code>
              <span>
                {snapshot.source === "legacy_backup"
                  ? "旧版 .bak（只读候选）"
                  : snapshot.pinned ? "固定基线" : "事务快照"}
                {" · "}{snapshot.app_version}
              </span>
            </div>
            <button
              className="btn tiny"
              type="button"
              disabled={busy || !snapshot.restorable}
              onClick={() => onRestore(snapshot)}
            >
              {snapshot.restorable ? "预览恢复" : "仅供人工核对"}
            </button>
          </article>
        ))}
      </div>
    </section>
  );
}
