import { useEffect, useMemo, useState } from "react";
import {
  applyAgentPlan,
  planAgentConnection,
  planAgentDisconnect,
  saveAgentRoutes,
  setAgentRouteMode,
  setAgentTier,
  type AgentInstallationView,
  type AgentRouteView,
  type AgentUiMetadataView,
  type AgentView,
  type ProviderView,
  type StateView,
  type TierSlot,
} from "../api";
import TierRouteEditor from "../components/TierRouteEditor";
import InstallationPicker from "../components/InstallationPicker";

interface AgentRoutePageProps {
  metadata: AgentUiMetadataView;
  agent?: AgentView;
  route: AgentRouteView;
  providers: ProviderView[];
  serveRunning: boolean;
  onStateChange: (state: StateView, message?: string) => void;
  onRescan: () => void | Promise<void>;
}

const AGENT_MARKS: Record<string, string> = {
  "claude-code": "C",
  codex: "X",
  opencode: "O",
  openclaw: "OC",
  "nous-hermes-agent": "H",
};

function errorText(error: unknown) {
  if (typeof error === "string") return error;
  if (error && typeof error === "object") {
    const value = error as { message?: unknown; code?: unknown; stage?: unknown };
    return [value.message, value.code && `code=${value.code}`, value.stage && `stage=${value.stage}`]
      .filter(Boolean)
      .map(String)
      .join(" · ");
  }
  return String(error);
}

function isExperimentalCompatibility(installation: AgentInstallationView | undefined) {
  if (!installation) return false;
  return ["DETECTED_INFERRED", "DETECTED_UNKNOWN"].includes(installation.compatibility.status)
    && Boolean(installation.discovery.version_normalized)
    && Boolean(installation.compatibility.connector_id)
    && installation.compatibility.allowed_actions.includes("run_read_only_preflight");
}

function statusCopy(agent: AgentView | undefined, installation: AgentInstallationView | undefined) {
  if (!agent || agent.installations.length === 0) return { tone: "idle", label: "未发现", detail: "没有在本机发现可管理的安装。" };
  if (installation?.connected) return { tone: "success", label: "已接入", detail: "请求已通过 Token Station。" };
  if (installation && ["DETECTED_BLOCKED", "INSTALLED_BROKEN"].includes(installation.compatibility.status)) return { tone: "danger", label: "暂不可接入", detail: "当前版本或配置未通过安全准入。" };
  if (isExperimentalCompatibility(installation)) return { tone: "warning", label: "版本未经验证", detail: "当前版本未在兼容目录中验证，可在确认风险后试验性接入。" };
  if (installation?.compatibility.status === "DETECTED_UNKNOWN") return { tone: "warning", label: "版本待确认", detail: "兼容目录尚未确认当前版本。" };
  return { tone: "ready", label: "可接入", detail: "已发现兼容安装，可以一键接入。" };
}

export default function AgentRoutePage({
  metadata,
  agent,
  route,
  providers,
  serveRunning,
  onStateChange,
  onRescan,
}: AgentRoutePageProps) {
  const [selectedPath, setSelectedPath] = useState("");
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState("");
  const [error, setError] = useState("");
  const [experimentalConfirmationOpen, setExperimentalConfirmationOpen] = useState(false);

  useEffect(() => {
    const paths = agent?.installations.map((item) => item.discovery.canonical_path) ?? [];
    setSelectedPath((current) => paths.includes(current) ? current : paths[0] ?? "");
  }, [agent]);

  const installation = useMemo(
    () => agent?.installations.find((item) => item.discovery.canonical_path === selectedPath),
    [agent, selectedPath],
  );
  useEffect(() => {
    setExperimentalConfirmationOpen(false);
  }, [
    metadata.agent_id,
    selectedPath,
    installation?.discovery.version_normalized,
    installation?.discovery.version_raw,
    installation?.compatibility.connector_id,
    installation?.compatibility.status,
  ]);

  const status = statusCopy(agent, installation);
  const connected = installation?.connected ?? false;
  const experimentalCompatibility = !connected && isExperimentalCompatibility(installation);
  const canConnect = Boolean(
    installation
      && (["DETECTED_VERIFIED", "CONNECTED"].includes(installation.compatibility.status)
        || experimentalCompatibility),
  );
  const canOperate = connected ? Boolean(installation) : serveRunning && canConnect;

  const runState = async (action: () => Promise<StateView>, message?: string) => {
    if (busy) return;
    setBusy(true);
    setError("");
    setNotice("");
    try {
      onStateChange(await action(), message);
      if (message) setNotice(message);
    } catch (caught) {
      setError(errorText(caught));
    } finally {
      setBusy(false);
    }
  };

  const applyConnection = async (experimentalConfirmed = false) => {
    if (!installation || !canOperate || busy) return;
    if (experimentalCompatibility && !experimentalConfirmed) {
      setExperimentalConfirmationOpen(true);
      return;
    }
    setExperimentalConfirmationOpen(false);
    setBusy(true);
    setError("");
    setNotice("");
    try {
      const plan = connected
        ? await planAgentDisconnect(metadata.agent_id, installation.discovery.canonical_path)
        : await planAgentConnection(
          metadata.agent_id,
          installation.discovery.canonical_path,
          experimentalCompatibility
            ? { expectedVersion: installation.discovery.version_normalized as string }
            : undefined,
        );
      await applyAgentPlan(
        plan.operation_id,
        plan.confirmation_token,
        experimentalCompatibility,
      );
      setNotice(connected
        ? "已恢复接入前的 Agent 配置"
        : experimentalCompatibility
          ? "Agent 已试验性接入"
          : "Agent 已接入，无需再次确认");
      await onRescan();
    } catch (caught) {
      setError(errorText(caught));
    } finally {
      setBusy(false);
    }
  };

  const switchMode = async (mode: "inherit" | "custom") => {
    await runState(() => setAgentRouteMode(metadata.agent_id, mode));
  };

  const saveRoute = () => runState(
    saveAgentRoutes,
    serveRunning ? "独立路由已保存 · 重启代理后生效" : "独立路由已保存",
  );

  const restoreHome = async () => {
    if (busy) return;
    setBusy(true);
    setError("");
    setNotice("");
    try {
      await setAgentRouteMode(metadata.agent_id, "inherit");
      const next = await saveAgentRoutes();
      onStateChange(next);
      setNotice(serveRunning ? "已恢复跟随主页 · 重启代理后生效" : "已恢复跟随主页");
    } catch (caught) {
      setError(errorText(caught));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="page-stack agent-route-page">
      <header className="agent-route-hero panel">
        <div className="agent-identity">
          <span className="agent-large-mark" aria-hidden="true">
            {AGENT_MARKS[metadata.agent_id] ?? metadata.display_name.slice(0, 1)}
          </span>
          <div>
            <span className="eyebrow">AGENT ROUTE</span>
            <h1>{metadata.display_name}</h1>
          </div>
        </div>
        <div className="agent-connect-box">
          <span className={`status-chip ${status.tone}`}>{status.label}</span>
          <small>{status.detail}</small>
          <InstallationPicker
            agentName={metadata.display_name}
            installations={agent?.installations ?? []}
            selectedPath={selectedPath}
            disabled={busy}
            onSelect={setSelectedPath}
          />
          <button
            className={`btn agent-primary-action ${connected ? "" : "primary"}`}
            type="button"
            disabled={busy || !canOperate}
            onClick={() => void applyConnection()}
          >
            {busy ? "处理中…" : connected ? "恢复 Agent 原始配置" : experimentalCompatibility ? "试验性接入" : "一键接入"}
          </button>
        </div>
      </header>

      {experimentalConfirmationOpen && experimentalCompatibility && installation && (
        <div
          className="agent-dialog-backdrop"
          onKeyDown={(event) => {
            if (event.key === "Escape") setExperimentalConfirmationOpen(false);
          }}
        >
          <section
            className="agent-dialog"
            role="alertdialog"
            aria-modal="true"
            aria-labelledby="experimental-connect-title"
            aria-describedby="experimental-connect-risk"
          >
            <div className="agent-dialog-head">
              <div>
                <span className="eyebrow">UNVERIFIED VERSION</span>
                <h2 id="experimental-connect-title">确认试验性接入</h2>
              </div>
            </div>
            <p id="experimental-connect-risk" className="experimental-warning">
              {metadata.display_name} {installation.discovery.version_normalized ?? installation.discovery.version_raw ?? "未知版本"}
              尚未通过兼容目录验证。继续操作可能因上游配置格式变化而失败。
            </p>
            <div className="plan-bindings">
              <div>
                <span>安装路径</span>
                <code>{installation.discovery.canonical_path}</code>
              </div>
            </div>
            <p>写入前会创建快照；失败时会自动恢复，不会绕过配置预检或安全准入。</p>
            <div className="agent-dialog-actions">
              <button autoFocus className="btn" type="button" disabled={busy} onClick={() => setExperimentalConfirmationOpen(false)}>取消</button>
              <button className="btn primary" type="button" disabled={busy} onClick={() => void applyConnection(true)}>确认试验性接入</button>
            </div>
          </section>
        </div>
      )}

      {!serveRunning && !connected && <div className="inline-note">请先启动代理，再接入 Agent。路由仍可先行配置。</div>}
      {notice && <div className="banner ok">{notice}</div>}
      {error && <div className="banner err">{error}</div>}

      <section className="panel route-panel">
        <div className="panel-head split-heading">
          <div>
            <span className="eyebrow">ROUTING PROFILE</span>
            <h2>三档路由</h2>
            <p className="sub">供应商和模型选择与主页完全一致；只决定此 Agent 是否使用独立配置。</p>
          </div>
          <div className="mode-switch" role="radiogroup" aria-label="Agent 路由模式">
            <button type="button" role="radio" aria-checked={route.mode === "inherit"} className={route.mode === "inherit" ? "active" : ""} disabled={busy} onClick={() => void switchMode("inherit")}>跟随主页</button>
            <button type="button" role="radio" aria-checked={route.mode === "custom"} className={route.mode === "custom" ? "active" : ""} disabled={busy} onClick={() => void switchMode("custom")}>独立路由</button>
          </div>
        </div>

        <TierRouteEditor
          tiers={route.tiers}
          providers={providers}
          disabled={busy}
          readOnly={route.mode === "inherit"}
          onTierChange={(slot: TierSlot, upstream, model) => runState(() => setAgentTier(metadata.agent_id, slot, upstream, model))}
        />

        <footer className="panel-foot route-actions">
          {route.mode === "custom" ? (
            <>
              <button className="btn primary" type="button" disabled={busy || Boolean(route.config_error)} onClick={() => void saveRoute()}>保存独立路由</button>
              <button className="btn" type="button" disabled={busy} onClick={() => void restoreHome()}>恢复主页路由</button>
            </>
          ) : (
            <span className="inherit-note">主页路由更新后，此 Agent 会自动使用最新三档配置。</span>
          )}
          {route.config_error && <span className="foot-hint error-text">{route.config_error}</span>}
        </footer>
      </section>
    </div>
  );
}
