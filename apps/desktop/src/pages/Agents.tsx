import { useCallback, useEffect, useMemo, useState } from "react";
import {
  applyAgentPlan,
  applySnapshotRestore,
  listAgentSnapshots,
  planAgentConnection,
  planAgentDisconnect,
  planSnapshotRestore,
  scanAgents,
  type AgentInstallationView,
  type AgentView,
  type ConfigPlanView,
  type SnapshotView,
} from "../api";
import AgentCard from "../components/AgentCard";
import AgentChangePreview from "../components/AgentChangePreview";
import AgentSnapshotList from "../components/AgentSnapshotList";

interface AgentsProps {
  serveRunning: boolean;
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

export default function Agents({ serveRunning }: AgentsProps) {
  const [agents, setAgents] = useState<AgentView[]>([]);
  const [selected, setSelected] = useState<Record<string, string>>({});
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [plan, setPlan] = useState<ConfigPlanView | null>(null);
  const [planError, setPlanError] = useState("");
  const [snapshotAgent, setSnapshotAgent] = useState<AgentView | null>(null);
  const [snapshots, setSnapshots] = useState<SnapshotView[]>([]);
  const [snapshotLoading, setSnapshotLoading] = useState(false);
  const [snapshotError, setSnapshotError] = useState("");

  const rescan = useCallback(async () => {
    setLoading(true);
    setError("");
    try {
      const next = await scanAgents();
      setAgents(next);
      setSelected((current) => {
        const reconciled = { ...current };
        for (const agent of next) {
          const paths = agent.installations.map((item) => item.discovery.canonical_path);
          if (paths.length === 1) reconciled[agent.metadata.agent_id] = paths[0];
          else if (!paths.includes(reconciled[agent.metadata.agent_id])) reconciled[agent.metadata.agent_id] = "";
        }
        return reconciled;
      });
    } catch (caught) {
      setError(errorText(caught));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void rescan();
  }, [rescan]);

  const counts = useMemo(
    () => ({
      detected: agents.filter((agent) => agent.installations.length > 0).length,
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
      setPlan(null);
      await rescan();
      if (snapshotAgent) await showSnapshots(snapshotAgent);
    } catch (caught) {
      setPlanError(errorText(caught));
    } finally {
      setBusy(false);
    }
  };

  return (
    <main className="agents-page">
      <header className="agents-hero">
        <div>
          <span className="agents-eyebrow">LOCAL AGENT CONTROL PLANE</span>
          <h1>Agent 接入站</h1>
          <p>自动识别安装与版本；真正写配置前，先看证据、差异和恢复点。</p>
        </div>
        <div className="agents-summary" aria-label="Agent 扫描摘要">
          <div><strong>{agents.length || 5}</strong><span>Registry</span></div>
          <div><strong>{counts.detected}</strong><span>已检测</span></div>
          <div><strong>{counts.connected}</strong><span>已接入</span></div>
          <button className="btn" type="button" disabled={loading || busy} onClick={() => void rescan()}>
            {loading ? "扫描中…" : "重新扫描"}
          </button>
        </div>
      </header>

      <div className="agent-safety-strip">
        <span>只读扫描</span><i />
        <span>版本保护</span><i />
        <span>差异确认</span><i />
        <span>加密快照</span><i />
        <span>原子写入</span>
      </div>

      {!serveRunning && (
        <div className="agent-page-note">代理当前未启动：扫描与快照仍可用，接入按钮保持禁用。</div>
      )}
      {notice && <div className="banner ok">{notice}</div>}
      {error && <div className="banner err">{error}</div>}

      {loading && agents.length === 0 ? (
        <div className="agent-page-empty">正在只读扫描本机 Agent…</div>
      ) : agents.length === 0 && !error ? (
        <div className="agent-page-empty">Registry 没有可展示的 Agent。</div>
      ) : (
        <section className="agent-card-grid" aria-label="Agent 列表">
          {agents.map((agent) => (
            <AgentCard
              key={agent.metadata.agent_id}
              agent={agent}
              selectedPath={selected[agent.metadata.agent_id] ?? ""}
              busy={busy}
              serveRunning={serveRunning}
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
