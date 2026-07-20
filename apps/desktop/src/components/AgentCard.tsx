import type { AgentInstallationView, AgentStatus, AgentView } from "../api";

const STATUS_COPY: Record<AgentStatus, { label: string; tone: string }> = {
  NOT_DETECTED: { label: "未安装", tone: "quiet" },
  DETECTED_VERIFIED: { label: "可接入", tone: "ready" },
  DETECTED_INFERRED: { label: "推定兼容", tone: "caution" },
  DETECTED_UNKNOWN: { label: "版本待验证", tone: "caution" },
  DETECTED_BLOCKED: { label: "已阻止", tone: "blocked" },
  INSTALLED_BROKEN: { label: "只读保护", tone: "blocked" },
  MULTIPLE_INSTALLATIONS: { label: "需选择", tone: "caution" },
  CONNECTED: { label: "已接入", tone: "connected" },
};

const ICONS: Record<string, string> = {
  claude: "CC",
  codex: "CX",
  opencode: "O",
  openclaw: "OC",
  hermes: "H",
};

interface AgentCardProps {
  agent: AgentView;
  selectedPath: string;
  expanded: boolean;
  busy: boolean;
  serveRunning: boolean;
  onToggle: () => void;
  onSelect: (path: string) => void;
  onPlanConnect: (installation: AgentInstallationView) => void;
  onPlanDisconnect: (installation: AgentInstallationView) => void;
  onViewSnapshots: () => void;
}

export default function AgentCard({
  agent,
  selectedPath,
  expanded,
  busy,
  serveRunning,
  onToggle,
  onSelect,
  onPlanConnect,
  onPlanDisconnect,
  onViewSnapshots,
}: AgentCardProps) {
  const selected = agent.installations.find(
    (installation) => installation.discovery.canonical_path === selectedPath,
  );
  const status = selected?.connected
    ? "CONNECTED"
    : selected?.compatibility.status ?? agent.status;
  const statusCopy = STATUS_COPY[status];
  const canConnect =
    !!selected &&
    !selected.connected &&
    agent.metadata.admission === "supported" &&
    ["DETECTED_VERIFIED", "DETECTED_INFERRED", "MULTIPLE_INSTALLATIONS"].includes(
      selected.compatibility.status,
    );
  const version = selected?.discovery.version_normalized ?? selected?.discovery.version_raw;
  const reason =
    selected?.compatibility.message ??
    (agent.installations.length === 0
      ? "本机未找到可运行入口。Token Station 不会自动安装或升级它。"
      : "请选择一个安装实例后继续。");
  const source = selected
    ? selected.discovery.is_path_default
      ? "PATH 默认"
      : "已发现路径"
    : agent.installations.length > 1
      ? `检测到 ${agent.installations.length} 个`
      : "未检测到";
  const versionSummary =
    version ?? (agent.installations.length > 1 ? "请选择安装实例" : "—");
  const detailsId = `agent-details-${agent.metadata.agent_id}`;

  return (
    <article
      className={`agent-list-item tone-${statusCopy.tone} ${expanded ? "expanded" : ""}`}
      data-testid={`agent-${agent.metadata.agent_id}`}
      role="listitem"
    >
      <button
        className="agent-row-toggle"
        type="button"
        aria-expanded={expanded}
        aria-controls={detailsId}
        onClick={onToggle}
      >
        <span className="agent-identity">
          <span className="agent-mark" aria-hidden="true">
            {ICONS[agent.metadata.icon_key] ?? agent.metadata.display_name.slice(0, 1)}
          </span>
          <span className="agent-name-block">
            <strong>{agent.metadata.display_name}</strong>
            <span>{agent.metadata.agent_id}</span>
          </span>
        </span>
        <span className="agent-row-version" title={versionSummary}>
          {versionSummary}
        </span>
        <span className="agent-row-source" title={source}>
          {source}
        </span>
        <span className="agent-row-state">
          <span className={`agent-status ${statusCopy.tone}`}>{statusCopy.label}</span>
          <span className="agent-chevron" aria-hidden="true">⌄</span>
        </span>
      </button>

      {expanded && (
        <div className="agent-row-details" id={detailsId}>
          <section className="agent-detail-section">
            <h3>{agent.installations.length > 1 ? "选择安装实例" : "当前安装"}</h3>
            {agent.installations.length > 1 ? (
              <label className="agent-instance-field">
                <span>真实路径</span>
                <select
                  aria-label={`${agent.metadata.display_name} 安装实例`}
                  value={selectedPath}
                  disabled={busy}
                  onChange={(event) => onSelect(event.target.value)}
                >
                  <option value="">请选择真实路径</option>
                  {agent.installations.map((installation) => (
                    <option
                      key={installation.discovery.canonical_path}
                      value={installation.discovery.canonical_path}
                    >
                      {installation.discovery.canonical_path}
                    </option>
                  ))}
                </select>
              </label>
            ) : (
              <div className="agent-path">
                <span>真实路径</span>
                <code>{selected?.discovery.canonical_path ?? "—"}</code>
              </div>
            )}
            <p className={`agent-reason ${statusCopy.tone}`}>{reason}</p>
            {!serveRunning && canConnect && (
              <p className="agent-action-hint">启动代理后才能生成接入计划。</p>
            )}
          </section>

          <section className="agent-detail-section">
            <h3>接入与恢复</h3>
            <dl className="agent-detail-facts">
              <div>
                <dt>配置候选</dt>
                <dd title={selected?.discovery.config_candidates[0]}>
                  {selected?.discovery.config_candidates[0] ?? "—"}
                </dd>
              </div>
              <div>
                <dt>目录序列</dt>
                <dd>seq {agent.catalog_sequence}</dd>
              </div>
              <div>
                <dt>连接状态</dt>
                <dd>{selected?.connected ? "已接入" : "未接入"}</dd>
              </div>
            </dl>
            <footer className="agent-card-actions">
              {selected?.connected ? (
                <button
                  className="btn danger"
                  type="button"
                  disabled={busy}
                  onClick={() => onPlanDisconnect(selected)}
                >
                  预览断开
                </button>
              ) : (
                <button
                  className="btn primary"
                  type="button"
                  disabled={busy || !serveRunning || !canConnect}
                  onClick={() => selected && onPlanConnect(selected)}
                >
                  {status === "DETECTED_INFERRED" ? "预检并预览" : "预览接入"}
                </button>
              )}
              <button className="btn quiet" type="button" disabled={busy} onClick={onViewSnapshots}>
                查看快照
              </button>
            </footer>
          </section>
        </div>
      )}
    </article>
  );
}
