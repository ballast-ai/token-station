import type { AgentInstallationView, AgentStatus, AgentView } from "../api";

const STATUS_COPY: Record<AgentStatus, { label: string; tone: string }> = {
  NOT_DETECTED: { label: "未检测到", tone: "quiet" },
  DETECTED_VERIFIED: { label: "版本已验证", tone: "ready" },
  DETECTED_INFERRED: { label: "推定兼容", tone: "caution" },
  DETECTED_UNKNOWN: { label: "版本未知", tone: "quiet" },
  DETECTED_BLOCKED: { label: "已阻断", tone: "blocked" },
  INSTALLED_BROKEN: { label: "安装异常", tone: "blocked" },
  MULTIPLE_INSTALLATIONS: { label: "多个实例", tone: "caution" },
  CONNECTED: { label: "已接入", tone: "connected" },
};

const ICONS: Record<string, string> = {
  claude: "A",
  codex: "C",
  opencode: "O",
  openclaw: "OC",
  hermes: "H",
};

interface AgentCardProps {
  agent: AgentView;
  selectedPath: string;
  busy: boolean;
  serveRunning: boolean;
  onSelect: (path: string) => void;
  onPlanConnect: (installation: AgentInstallationView) => void;
  onPlanDisconnect: (installation: AgentInstallationView) => void;
  onViewSnapshots: () => void;
}

export default function AgentCard({
  agent,
  selectedPath,
  busy,
  serveRunning,
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
  const reason = selected?.compatibility.message ??
    (agent.installations.length === 0
      ? "本机未找到可运行入口。Token Station 不会自动安装或升级它。"
      : "请选择一个安装实例后继续。");

  return (
    <article className={`agent-card tone-${statusCopy.tone}`} data-testid={`agent-${agent.metadata.agent_id}`}>
      <div className="agent-card-rail" aria-hidden="true">
        <span className="rail-node" />
        <span className="rail-line" />
      </div>
      <div className="agent-card-body">
        <header className="agent-card-head">
          <div className="agent-mark" aria-hidden="true">
            {ICONS[agent.metadata.icon_key] ?? agent.metadata.display_name.slice(0, 1)}
          </div>
          <div className="agent-card-title">
            <h2>{agent.metadata.display_name}</h2>
            <span className="agent-id">{agent.metadata.agent_id}</span>
          </div>
          <span className={`agent-status ${statusCopy.tone}`}>{statusCopy.label}</span>
        </header>

        {agent.installations.length > 1 && (
          <label className="agent-instance-field">
            <span>安装实例</span>
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
        )}

        <dl className="agent-facts">
          <div>
            <dt>版本</dt>
            <dd>{version ?? "—"}</dd>
          </div>
          <div>
            <dt>来源</dt>
            <dd>
              {selected
                ? selected.discovery.is_path_default
                  ? "PATH 默认"
                  : "已发现路径"
                : "—"}
            </dd>
          </div>
          <div>
            <dt>目录</dt>
            <dd>seq {agent.catalog_sequence}</dd>
          </div>
        </dl>

        <p className={`agent-reason ${statusCopy.tone}`}>{reason}</p>
        {!serveRunning && canConnect && (
          <p className="agent-action-hint">启动代理后才能生成接入计划。</p>
        )}

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
            快照
          </button>
        </footer>
      </div>
    </article>
  );
}
