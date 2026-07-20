import { useEffect, useMemo, useState } from "react";
import {
  type AgentPlanIntent,
  applyAgentPlan,
  applySnapshotRestore,
  listAgentSnapshots,
  planAgentConnection,
  planAgentDisconnect,
  planSnapshotRestore,
  type AgentInstallationView,
  type AgentUiMetadataView,
  type AgentView,
  type ConfigPlanView,
  type SnapshotView,
} from "../api";
import AgentCard from "../components/AgentCard";
import AgentChangePreview from "../components/AgentChangePreview";
import AgentSnapshotList from "../components/AgentSnapshotList";

interface AgentsProps {
  serveRunning: boolean;
  registry: AgentUiMetadataView[];
  agents: AgentView[];
  scanPhase: "idle" | "scanning" | "ready" | "error";
  scanError: string;
  lastScanAtMs: number | null;
  onRescan: () => void | Promise<void>;
  onOperationApplied: (
    intent: AgentPlanIntent,
    agentId: string,
    installationPath: string,
  ) => void;
}
function errorText(error: unknown): string {
  if (typeof error === "string") return error;
  if (error && typeof error === "object") {
    const value = error as { message?: unknown; code?: unknown; stage?: unknown; recovery?: unknown };
    const parts = [value.message, value.code && `code=${value.code}`, value.stage && `stage=${value.stage}`, value.recovery && `recovery=${value.recovery}`]
      .filter(Boolean)
      .map(String);
    if (parts.length) return parts.join(" · ");
  }
  return String(error);
}

export default function Agents({
  serveRunning,
  registry,
  agents,
  scanPhase,
  scanError,
  lastScanAtMs,
  onRescan,
  onOperationApplied,
}: AgentsProps) {
  const [selected, setSelected] = useState<Record<string, string>>({});
  const [expandedAgentId, setExpandedAgentId] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [plan, setPlan] = useState<ConfigPlanView | null>(null);
  const [planError, setPlanError] = useState("");
  const [snapshotAgent, setSnapshotAgent] = useState<AgentView | null>(null);
  const [snapshots, setSnapshots] = useState<SnapshotView[]>([]);
  const [snapshotLoading, setSnapshotLoading] = useState(false);
  const [snapshotError, setSnapshotError] = useState("");

  useEffect(() => {
    setExpandedAgentId((current) =>
      current && registry.some((agent) => agent.agent_id === current) ? current : null,
    );
    setSelected((current) => {
      const reconciled = { ...current };
      for (const agent of agents) {
        const paths = agent.installations.map((item) => item.discovery.canonical_path);
        if (paths.length === 1) reconciled[agent.metadata.agent_id] = paths[0];
        else if (!paths.includes(reconciled[agent.metadata.agent_id])) reconciled[agent.metadata.agent_id] = "";
      }
      return reconciled;
    });
  }, [agents, registry]);

  const displayAgents = useMemo(() => {
    const scanned = new Map(agents.map((agent) => [agent.metadata.agent_id, agent]));
    const rows = registry.map(
      (metadata): AgentView =>
        scanned.get(metadata.agent_id) ?? {
          metadata,
          installations: [],
          status: "NOT_DETECTED",
          catalog_sequence: 0,
          catalog_expires_at_ms: null,
          catalog_source: "builtin",
          catalog_warning: null,
        },
    );
    const registryIds = new Set(registry.map((metadata) => metadata.agent_id));
    rows.push(...agents.filter((agent) => !registryIds.has(agent.metadata.agent_id)));
    return rows;
  }, [agents, registry]);

  const counts = useMemo(
    () => ({
      detected: agents.filter((agent) => agent.installations.length > 0).length,
      ready: agents.filter((agent) =>
        agent.installations.some((installation) =>
          ["DETECTED_VERIFIED", "DETECTED_INFERRED"].includes(installation.compatibility.status),
        ),
      ).length,
      connected: agents.filter((agent) => agent.status === "CONNECTED").length,
    }),
    [agents],
  );

  const createPlan = async (action: () => Promise<ConfigPlanView>) => {
    if (busy) return;
    setBusy(true);
    setError("");
    setNotice("");
    setPlanError("");
    try {
      setPlan(await action());
    } catch (caught) {
      setError(errorText(caught));
    } finally {
      setBusy(false);
    }
  };

  const showSnapshots = async (agent: AgentView) => {
    setSnapshotAgent(agent);
    setSnapshotLoading(true);
    setSnapshotError("");
    try {
      setSnapshots(await listAgentSnapshots(agent.metadata.agent_id));
    } catch (caught) {
      setSnapshots([]);
      setSnapshotError(errorText(caught));
    } finally {
      setSnapshotLoading(false);
    }
  };

  const applyPlan = async () => {
    if (!plan || busy) return;
    setBusy(true);
    setPlanError("");
    try {
      const result =
        plan.intent === "restore"
          ? await applySnapshotRestore(plan.operation_id, plan.confirmation_token)
          : await applyAgentPlan(plan.operation_id, plan.confirmation_token);
      setNotice(
        plan.intent === "connect"
          ? "Agent 已接入"
          : plan.intent === "disconnect"
            ? "Agent 已安全断开"
            : "受管字段已从快照恢复",
      );
      if (result.maintenance_warning) setNotice(result.maintenance_warning);
      onOperationApplied(plan.intent, plan.agent_id, plan.installation_path);
      setPlan(null);
      if (snapshotAgent) await showSnapshots(snapshotAgent);
    } catch (caught) {
      setPlanError(errorText(caught));
    } finally {
      setBusy(false);
    }
  };

  return (
    <main className="agents-page">
      <section className="agents-toolbar panel">
        <header className="agents-heading">
          <div>
            <h1>Agent 管理</h1>
            <p>自动发现本机客户端；修改配置前先预览差异并创建恢复点。</p>
          </div>
          <button
            className="btn"
            type="button"
            disabled={scanPhase === "scanning" || busy}
            onClick={() => void onRescan()}
          >
            {scanPhase === "scanning" ? "扫描中…" : "重新扫描"}
          </button>
        </header>
        <div className="agents-summary" aria-label="Agent 扫描摘要">
          <span><strong>{counts.detected}</strong>已发现</span>
          <span><strong>{counts.ready}</strong>可接入</span>
          <span><strong>{counts.connected}</strong>已接入</span>
          <span className="agents-readonly-state">
            {scanPhase === "scanning"
              ? lastScanAtMs == null
                ? "后台扫描中"
                : "正在重新扫描"
              : scanPhase === "error"
                ? "结果可能已过期"
                : lastScanAtMs == null
                  ? "等待扫描"
                  : `最近扫描 ${new Date(lastScanAtMs).toLocaleTimeString()}`}
          </span>
        </div>
      </section>

      {!serveRunning && (
        <div className="agent-page-note">代理当前未启动：扫描与快照仍可用，接入按钮保持禁用。</div>
      )}
      {notice && <div className="banner ok">{notice}</div>}
      {scanError && <div className="banner err">{scanError}</div>}
      {error && <div className="banner err">{error}</div>}

      {displayAgents.length === 0 && !scanError ? (
        <div className="agent-page-empty">Registry 没有可展示的 Agent。</div>
      ) : (
        <section className="agent-list panel" role="list" aria-label="Agent 列表">
          <div className="agent-list-head" aria-hidden="true">
            <span>客户端</span>
            <span>版本</span>
            <span>安装来源</span>
            <span>状态</span>
          </div>
          {displayAgents.map((agent) => (
            <AgentCard
              key={agent.metadata.agent_id}
              agent={agent}
              selectedPath={selected[agent.metadata.agent_id] ?? ""}
              expanded={expandedAgentId === agent.metadata.agent_id}
              scanCompleted={lastScanAtMs != null}
              busy={busy || scanPhase === "scanning"}
              serveRunning={serveRunning}
              onToggle={() =>
                setExpandedAgentId((current) =>
                  current === agent.metadata.agent_id ? null : agent.metadata.agent_id,
                )
              }
              onSelect={(path) => setSelected((current) => ({ ...current, [agent.metadata.agent_id]: path }))}
              onPlanConnect={(installation: AgentInstallationView) =>
                void createPlan(() =>
                  planAgentConnection(agent.metadata.agent_id, installation.discovery.canonical_path),
                )
              }
              onPlanDisconnect={(installation: AgentInstallationView) =>
                void createPlan(() =>
                  planAgentDisconnect(agent.metadata.agent_id, installation.discovery.canonical_path),
                )
              }
              onViewSnapshots={() => void showSnapshots(agent)}
            />
          ))}
        </section>
      )}

      {snapshotAgent && (
        <AgentSnapshotList
          agentName={snapshotAgent.metadata.display_name}
          snapshots={snapshots}
          loading={snapshotLoading}
          error={snapshotError}
          busy={busy}
          onClose={() => setSnapshotAgent(null)}
          onRestore={(snapshot) => void createPlan(() => planSnapshotRestore(snapshot.snapshot_id))}
        />
      )}

      {plan && (
        <AgentChangePreview
          plan={plan}
          busy={busy}
          error={planError}
          onCancel={() => {
            setPlan(null);
            setPlanError("");
          }}
          onApply={() => void applyPlan()}
        />
      )}
    </main>
  );
}
