import { useEffect, useMemo, useState } from "react";
import {
  applyAgentPlan,
  getAgentDrift,
  mountAgentProfile,
  planAgentConnection,
  planAgentDisconnect,
  saveAgentRoutes,
  setAgentRouteMode,
  setAgentTier,
  type AgentInstallationView,
  type AgentDriftView,
  type ConfigPlanView,
  type AgentRouteView,
  type AgentUiMetadataView,
  type AgentView,
  type ProviderView,
  type StateView,
  type TierSlot,
} from "../api";
import TierRouteEditor from "../components/TierRouteEditor";
import AgentDriftPanel from "../components/AgentDriftPanel";
import InstallationPicker from "../components/InstallationPicker";

interface AgentRoutePageProps {
  metadata: AgentUiMetadataView;
  agent?: AgentView;
  route: AgentRouteView;
  providers: ProviderView[];
  profiles: string[];
  serveRunning: boolean;
  onStateChange: (state: StateView, message?: string) => void;
  onRescan: () => void | Promise<void>;
}

const BINARY_SOURCE_LABELS: Record<AgentInstallationView["discovery"]["binary_source"], string> = {
  homebrew: "Homebrew",
  npm_global: "npm 全局",
  path: "PATH",
  known_path: "已知目录",
  env_override: "环境变量",
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

function statusCopy(agent: AgentView | undefined, installation: AgentInstallationView | undefined) {
  if (!agent || agent.installations.length === 0) return { tone: "idle", label: "未发现", detail: "没有在本机发现可管理的安装。" };
  if (agent.installations.length > 1 && !installation) {
    return { tone: "idle", label: "待选择", detail: "检测到多份安装，请先选择要接管的精确路径。" };
  }
  if (installation?.connected) return { tone: "success", label: "已接入", detail: "请求已通过 Token Station。" };
  if (isExactMultiInstallSelection(agent, installation)) {
    return { tone: "ready", label: "可接入", detail: "已选择精确安装，可以一键接入。" };
  }
  if (installation && installation.compatibility.status !== "DETECTED_VERIFIED") return { tone: "danger", label: "暂不可接入", detail: installation.compatibility.message };
  return { tone: "ready", label: "可接入", detail: "已发现兼容安装，可以一键接入。" };
}

function isExactMultiInstallSelection(
  agent: AgentView | undefined,
  installation: AgentInstallationView | undefined,
) {
  return Boolean(
    agent
      && agent.installations.length > 1
      && installation
      && installation.compatibility.status === "MULTIPLE_INSTALLATIONS"
      && installation.discovery.conflict_group,
  );
}

export default function AgentRoutePage({
  metadata,
  agent,
  route,
  providers,
  profiles,
  serveRunning,
  onStateChange,
  onRescan,
}: AgentRoutePageProps) {
  const [selectedPath, setSelectedPath] = useState("");
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState("");
  const [error, setError] = useState("");
  const [drift, setDrift] = useState<AgentDriftView[] | null>(null);
  const [driftLoading, setDriftLoading] = useState(false);
  const [driftError, setDriftError] = useState("");
  const [pendingPlan, setPendingPlan] = useState<ConfigPlanView | null>(null);

  useEffect(() => {
    const paths = agent?.installations.map((item) => item.discovery.canonical_path) ?? [];
    setSelectedPath((current) => paths.includes(current) ? current : paths.length === 1 ? paths[0] : "");
  }, [agent]);

  const installation = useMemo(
    () => agent?.installations.find((item) => item.discovery.canonical_path === selectedPath),
    [agent, selectedPath],
  );

  useEffect(() => {
    if (!installation) {
      setDrift(null);
      setDriftError("");
      setDriftLoading(false);
      return;
    }
    let cancelled = false;
    setDriftLoading(true);
    setDriftError("");
    void getAgentDrift(metadata.agent_id, installation.discovery.canonical_path)
      .then((views) => {
        if (!cancelled) setDrift(views);
      })
      .catch((caught) => {
        if (!cancelled) {
          setDrift(null);
          setDriftError(errorText(caught));
        }
      })
      .finally(() => {
        if (!cancelled) setDriftLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [installation, metadata.agent_id]);
  const status = statusCopy(agent, installation);
  const connected = installation?.connected ?? false;
  const canConnect = Boolean(
    installation
      && (["DETECTED_VERIFIED", "CONNECTED"].includes(installation.compatibility.status)
        || isExactMultiInstallSelection(agent, installation)),
  );
  const canOperate = connected ? Boolean(installation) : serveRunning && canConnect;

  const runState = async (action: () => Promise<StateView>, message?: string) => {
    if (busy) return;
    setBusy(true);
    setError("");
    setNotice("");
    try {
      onStateChange(await action());
      if (message) setNotice(message);
    } catch (caught) {
      setError(errorText(caught));
    } finally {
      setBusy(false);
    }
  };

  const applyConnection = async () => {
    if (!installation || !canOperate || busy) return;
    setBusy(true);
    setError("");
    setNotice("");
    try {
      const plan = connected
        ? await planAgentDisconnect(metadata.agent_id, installation.discovery.canonical_path)
        : await planAgentConnection(
          metadata.agent_id,
          installation.discovery.canonical_path,
          installation.discovery.version_normalized
            ? { expectedVersion: installation.discovery.version_normalized as string }
            : undefined,
        );
      setPendingPlan(plan);
    } catch (caught) {
      setError(errorText(caught));
    } finally {
      setBusy(false);
    }
  };

  const confirmProjection = async () => {
    if (!pendingPlan || busy) return;
    setBusy(true);
    setError("");
    setNotice("");
    try {
      await applyAgentPlan(pendingPlan.operation_id, pendingPlan.confirmation_token);
      const restored = pendingPlan.intent !== "connect";
      setPendingPlan(null);
      setNotice(restored ? "已恢复接入前的 Agent 配置" : "Agent 已接入");
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

  const mountProfile = async (profile = route.profile ?? profiles[0]) => {
    if (!profile) {
      setError("还没有可挂载的策略组，请先在主页将三档路由另存为策略组。");
      return;
    }
    await runState(
      () => mountAgentProfile(metadata.agent_id, profile),
      `已挂载策略组「${profile}」· 尚待保存并应用`,
    );
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

  const copyUpgradeCommand = async () => {
    const command = installation?.discovery.upgrade_command;
    if (!command) return;
    try {
      await navigator.clipboard.writeText(command);
      setNotice("升级命令已复制；Token Station 不会自动执行");
      setError("");
    } catch (caught) {
      setError(`复制升级命令失败：${errorText(caught)}`);
    }
  };

  return (
    <div className="page-stack agent-route-page">
      <header className="agent-route-hero panel">
        <div className="agent-identity">
          <span className="agent-large-mark" aria-hidden="true">
            {metadata.nav_mark ?? metadata.display_name.slice(0, 1)}
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
            {busy ? "处理中…" : connected ? "恢复 Agent 原始配置" : "一键接入"}
          </button>
        </div>
      </header>

      {installation && (
        <section className="agent-installation-facts panel" aria-label="当前 Agent 安装诊断">
          <div className="agent-installation-facts-head">
            <div>
              <span className="eyebrow">INSTALLATION</span>
              <strong>{installation.discovery.is_path_default ? "当前 PATH 生效安装" : "已选择精确安装"}</strong>
            </div>
            <span>{BINARY_SOURCE_LABELS[installation.discovery.binary_source]}</span>
          </div>
          <code className="agent-installation-path">{installation.discovery.canonical_path}</code>
          <div className="agent-installation-metadata">
            <span>{installation.discovery.environment.toUpperCase()}</span>
            <span>{installation.discovery.modified_at_ms == null
              ? "修改时间未知"
              : `修改于 ${new Date(installation.discovery.modified_at_ms).toLocaleString()}`}</span>
            <span>{installation.discovery.binary_sha256
              ? `SHA-256 ${installation.discovery.binary_sha256}`
              : "SHA-256 不可读"}</span>
          </div>
          {installation.discovery.upgrade_command && (
            <div className="agent-upgrade-command">
              <code>{installation.discovery.upgrade_command}</code>
              <button className="btn tiny" type="button" disabled={busy} onClick={() => void copyUpgradeCommand()}>
                复制升级命令
              </button>
            </div>
          )}
        </section>
      )}

      {installation && <AgentDriftPanel views={drift} loading={driftLoading} error={driftError} />}

      {!serveRunning && !connected && <div className="inline-note">请先启动代理，再接入 Agent。路由仍可先行配置。</div>}
      {notice && <div className="banner ok">{notice}</div>}
      {error && <div className="banner err">{error}</div>}

      {pendingPlan && (
        <div className="projection-dialog-backdrop">
          <section
            className="panel projection-dialog"
            role="dialog"
            aria-modal="true"
            aria-label="配置投影预览"
          >
            <span className="eyebrow">CONNECTOR PROJECTION</span>
            <h2>配置投影预览</h2>
            <p>仅下列受管字段会变化；敏感值不会显示或进入前端计划。</p>
            <code className="agent-installation-path">{pendingPlan.target_config_path}</code>
            {(pendingPlan.related_config_paths ?? []).map((path) => (
              <code className="agent-installation-path" key={path}>{path}</code>
            ))}
            <pre className="projection-diff">{pendingPlan.human_diff || "没有字段变化"}</pre>
            <div className="projection-dialog-actions">
              <button className="btn" type="button" disabled={busy} onClick={() => setPendingPlan(null)}>
                取消
              </button>
              <button className="btn primary" type="button" disabled={busy} onClick={() => void confirmProjection()}>
                {busy ? "应用中…" : "确认并应用"}
              </button>
            </div>
          </section>
        </div>
      )}

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
            <button type="button" role="radio" aria-checked={route.mode === "profile"} className={route.mode === "profile" ? "active" : ""} disabled={busy} onClick={() => void mountProfile()}>挂载策略组</button>
          </div>
        </div>

        {route.mode === "profile" && (
          <label className="profile-mount-select">
            <span>当前策略组</span>
            <select
              className="select"
              value={route.profile ?? ""}
              disabled={busy}
              onChange={(event) => void mountProfile(event.target.value)}
            >
              {profiles.map((profile) => <option key={profile} value={profile}>{profile}</option>)}
            </select>
          </label>
        )}

        <TierRouteEditor
          tiers={route.tiers}
          providers={providers}
          disabled={busy}
          readOnly={route.mode !== "custom"}
          onTierChange={(slot: TierSlot, upstream, model) => runState(() => setAgentTier(metadata.agent_id, slot, upstream, model))}
        />

        <footer className="panel-foot route-actions">
          {route.mode === "custom" ? (
            <>
              <button className="btn primary" type="button" disabled={busy || Boolean(route.config_error)} onClick={() => void saveRoute()}>保存独立路由</button>
              <button className="btn" type="button" disabled={busy} onClick={() => void restoreHome()}>恢复主页路由</button>
            </>
          ) : route.mode === "profile" ? (
            <span className="inherit-note">该 Agent 使用策略组「{route.profile}」。在主页管理策略组，保存并应用后生效。</span>
          ) : (
            <span className="inherit-note">主页路由更新后，此 Agent 会自动使用最新三档配置。</span>
          )}
          {route.config_error && <span className="foot-hint error-text">{route.config_error}</span>}
        </footer>
      </section>
    </div>
  );
}
