import { useEffect, useMemo, useState } from "react";
import {
  applyAgentPlan,
  configureCursorProvider,
  forceForgetAgent,
  mountAgentProfile,
  planAgentConnection,
  saveAgentRoutes,
  restartAgentRoute,
  setAgentRouteMode,
  setAgentTier,
  type AgentInstallationView,
  type ConfigPlanView,
  type AgentRouteView,
  type AgentUiMetadataView,
  type AgentView,
  type ProviderView,
  type QuotaAccount,
  type StateView,
  type TierSlot,
} from "../api";
import TierRouteEditor from "../components/TierRouteEditor";
import InstallationPicker from "../components/InstallationPicker";
import QuotaPriorityPanel from "../components/QuotaPriorityPanel";
import RoutingModeSelector from "../components/RoutingModeSelector";
import { AgentIcon } from "../brandIcons";
import { useLocalizedCopy } from "../components/LanguageProvider";
import { humanizeAppError } from "../errors";

interface AgentRoutePageProps {
  metadata: AgentUiMetadataView;
  agent?: AgentView;
  route: AgentRouteView;
  providers: ProviderView[];
  profiles: string[];
  quotaAccounts: QuotaAccount[];
  serveRunning: boolean;
  applying: boolean;
  onStateChange: (state: StateView, message?: string) => void;
  onRescan: () => void | Promise<void>;
  onSaveQuota: (accounts: QuotaAccount[]) => void;
  onSaveQuotaPlan: (
    upstream: string,
    lenMs: number,
    limit: number,
    unit: "tokens" | "requests",
  ) => void;
  onViewQuotaUsage: () => void;
  onSetRoutingMode?: (mode: "tiered" | "quota_first") => void;
  onInstallationSelected?: () => void;
  embedded?: boolean;
}

/** Per-Agent key recording that connection changes were shown; localStorage makes it appear only once. */
const diffShownKey = (agentId: string) => `ts:agent-connect-diff-shown:${agentId}`;

/** Managed fields that best explain where data flows and therefore matter most to users. */
const KEY_CHANGE_HINT = /url|base|token|key|auth|endpoint|host|proxy/i;

function errorText(error: unknown) {
  return humanizeAppError(error);
}

function statusCopy(
  metadata: AgentUiMetadataView,
  agent: AgentView | undefined,
  installation: AgentInstallationView | undefined,
  copy: (english: string, simplifiedChinese: string) => string,
  language: "en" | "zh-CN",
) {
  if (!agent || agent.installations.length === 0) {
    return {
      tone: "idle",
      label: copy("Not found", "未发现"),
      detail: copy("No manageable installation was found on this device.", "没有在本机发现可管理的安装。"),
    };
  }
  if (agent.installations.length > 1 && !installation) {
    return {
      tone: "idle",
      label: copy("Select one", "待选择"),
      detail: copy(
        "Multiple installations were detected. Select the exact path to manage.",
        "检测到多份安装，请先选择要接管的精确路径。",
      ),
    };
  }
  if (installation?.adapter_ready === false) {
    return {
      tone: "danger",
      label: copy("Adapter unavailable", "适配器未就绪"),
      detail: installation.managed
        ? copy(
            "The managed configuration still exists, but the running Gateway did not load its required inbound adapter. Requests cannot be served; restore the original configuration or repair the adapter and restart the proxy.",
            "接管配置仍存在，但当前 Gateway 未加载所需入站适配器，无法处理请求。请恢复原始配置，或修复适配器后重启代理。",
          )
        : copy(
            "The running Gateway did not load the required inbound adapter. No Agent configuration was changed.",
            "当前 Gateway 未加载所需入站适配器，暂不可接入；Agent 配置未被修改。",
      ),
    };
  }
  if (installation?.compatibility.reason_code === "READ_ONLY_PREFLIGHT_FAILED") {
    return {
      tone: "danger",
      label: copy("Management state unavailable", "接管状态不可用"),
      detail: copy(
        "The Agent remains visible in read-only mode, but Token Station cannot verify its management records. Connect and recovery actions are disabled.",
        "Agent 仍可只读显示，但 Token Station 无法验证接管记录，接入和恢复操作已禁用。",
      ),
    };
  }
  if (installation?.connected) {
    return {
      tone: "success",
      label: copy("Connected", "已接入"),
      detail: copy("Requests are routed through Token Station.", "请求已通过 Token Station。"),
    };
  }
  if (metadata.agent_id === "cursor" && installation) {
    return {
      tone: "ready",
      label: copy("Ready", "可接入"),
      detail: copy(
        "Cursor settings will be backed up and configured automatically.",
        "运行中的 Cursor 不会被强制关闭。请退出 Cursor 后再点一键接入。",
      ),
    };
  }
  if (installation?.managed) {
    return {
      tone: "danger",
      label: copy("Repair needed", "需修复"),
      detail: copy(
        "A management record exists, but the runtime state does not match. Restore the original configuration before reconnecting.",
        "已有接管记录，但当前运行态不一致。请先恢复原始配置，再重新接入。",
      ),
    };
  }
  if (isExactMultiInstallSelection(agent, installation)) {
    return {
      tone: "ready",
      label: copy("Ready", "可接入"),
      detail: copy("The exact installation is selected and ready to connect.", "已选择精确安装，可以一键接入。"),
    };
  }
  if (installation && installation.compatibility.status !== "DETECTED_VERIFIED") {
    return {
      tone: "danger",
      label: copy("Unavailable", "暂不可接入"),
      detail: humanizeAppError({
        code: installation.compatibility.reason_code,
        message: installation.compatibility.message,
      }, language),
    };
  }
  return {
    tone: "ready",
    label: copy("Ready", "可接入"),
    detail: copy("A compatible installation was found and is ready to connect.", "已发现兼容安装，可以一键接入。"),
  };
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
  quotaAccounts,
  serveRunning,
  applying,
  onStateChange,
  onRescan,
  onSaveQuota,
  onSaveQuotaPlan,
  onViewQuotaUsage,
  onSetRoutingMode = () => {},
  onInstallationSelected,
  embedded = false,
}: AgentRoutePageProps) {
  const { copy, language } = useLocalizedCopy();
  const [selectedPath, setSelectedPath] = useState("");
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState("");
  const [error, setError] = useState("");
  // Show configuration changes after the first connection, then persist dismissal in localStorage.
  const [connectDiff, setConnectDiff] = useState<ConfigPlanView | null>(null);
  const dismissConnectDiff = () => setConnectDiff(null);

  useEffect(() => {
    const paths = agent?.installations.map((item) => item.discovery.canonical_path) ?? [];
    setSelectedPath((current) => paths.includes(current) ? current : paths.length === 1 ? paths[0] : "");
  }, [agent]);

  const installation = useMemo(
    () => agent?.installations.find((item) => item.discovery.canonical_path === selectedPath),
    [agent, selectedPath],
  );

  const status = statusCopy(metadata, agent, installation, copy, language);
  const managed = installation?.managed ?? false;
  const canConnect = Boolean(
    installation
      && installation.adapter_ready !== false
      && installation.compatibility.reason_code !== "READ_ONLY_PREFLIGHT_FAILED"
      && (metadata.agent_id === "cursor"
        || ["DETECTED_VERIFIED", "CONNECTED"].includes(installation.compatibility.status)
        || isExactMultiInstallSelection(agent, installation)),
  );
  const canOperate = managed ? Boolean(installation) : serveRunning && canConnect;

  const runState = async (action: () => Promise<StateView>, message?: string) => {
    if (busy) return;
    setBusy(true);
    setError("");
    setNotice("");
    try {
      onStateChange(await action());
      if (message) setNotice(message);
    } catch (caught) {
      const message = errorText(caught);
      setError(message.includes("cursor_running")
        ? copy(
          "Cursor is still running. Enter the TS API in Cursor settings, or quit Cursor yourself and click Connect again. Token Station will not close it for you.",
          "Cursor 仍在运行。你可以在 Cursor 设置中填写 TS API，或者自己退出 Cursor 后再点一次一键接入。Token Station 不会强制关闭它。",
        )
        : message);
    } finally {
      setBusy(false);
    }
  };

  // Connect immediately after planning without a redundant confirmation wait.
  // The post-connection diff card still appears once for transparency.
  const applyConnection = async () => {
    if (!installation || !canOperate || busy) return;
    setBusy(true);
    setError("");
    setNotice("");
    try {
      if (metadata.agent_id === "cursor") {
        const message = await configureCursorProvider();
        setNotice(message);
        await onRescan();
        return;
      }
      const plan = await planAgentConnection(
        metadata.agent_id,
        installation.discovery.canonical_path,
        installation.discovery.version_normalized
          ? { expectedVersion: installation.discovery.version_normalized as string }
          : undefined,
      );
      await applyAgentPlan(plan.operation_id, plan.confirmation_token);
      if (!localStorage.getItem(diffShownKey(metadata.agent_id))
        && (plan.changes?.length || plan.human_diff)) {
        localStorage.setItem(diffShownKey(metadata.agent_id), String(Date.now()));
        setConnectDiff(plan);
      }
      setNotice(copy("Agent connected.", "Agent 已接入"));
      await onRescan();
    } catch (caught) {
      const message = errorText(caught);
      setError(message.includes("cursor_running")
        ? copy(
          "Cursor is still running. Enter the TS API in Cursor settings, or quit Cursor yourself and click Connect again. Token Station will not close it for you.",
          "Cursor 仍在运行。你可以在 Cursor 设置中填写 TS API，或者自己退出 Cursor 后再点一次一键接入。Token Station 不会强制关闭它。",
        )
        : message);
    } finally {
      setBusy(false);
    }
  };

  // Restore official configuration and disconnect by removing TS-managed fields
  // according to ownership records, returning the Agent to official defaults,
  // then clearing ownership. This deterministic path replaces the old force-
  // disconnect fallback and does not depend on encrypted snapshots or a master key.
  const restoreOfficial = async () => {
    if (!installation || busy) return;
    setBusy(true);
    setError("");
    setNotice("");
    try {
      await forceForgetAgent(metadata.agent_id, installation.discovery.canonical_path);
      setNotice(copy(
        "Restored the official configuration and disconnected.",
        "已恢复官方配置并断开。",
      ));
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
      setError(copy(
        "No routing profile is available. Save the home routing configuration as a profile first.",
        "还没有可挂载的策略组，请先在主页将三档路由另存为策略组。",
      ));
      return;
    }
    await runState(
      () => mountAgentProfile(metadata.agent_id, profile),
      copy(
        `Routing profile “${profile}” mounted · Save and apply to finish`,
        `已挂载策略组「${profile}」· 尚待保存并应用`,
      ),
    );
  };

  // Save and hot-restart only this Agent's route when the proxy is running; do not affect other Agents.
  const saveRoute = () => runState(
    () => restartAgentRoute(metadata.agent_id),
    serveRunning
      ? copy("Custom routing saved and restarted for this Agent", "独立路由已保存并对此 Agent 生效")
      : copy("Custom routing saved", "独立路由已保存"),
  );

  // In Follow Home mode, apply the current home tiers to this Agent immediately.
  // Restarting an inherited route clears the Agent-specific route and hot-applies the home configuration.
  const applyHomeRoute = () => runState(
    () => restartAgentRoute(metadata.agent_id),
    serveRunning
      ? copy("Home routing applied to this Agent", "已将主页路由应用到此 Agent")
      : copy("Home routing saved · applies when the proxy starts", "已保存 · 启动代理后生效"),
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
      setNotice(serveRunning
        ? copy("Restored home routing · Restart the proxy to apply", "已恢复跟随主页 · 重启代理后生效")
        : copy("Restored home routing", "已恢复跟随主页"));
    } catch (caught) {
      setError(errorText(caught));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className={`page-stack agent-route-page ${embedded ? "agent-route-embedded" : ""}`}>
      <header className="agent-route-hero panel">
        <div className="agent-identity">
          <span className="agent-large-mark" aria-hidden="true">
            <AgentIcon
              id={metadata.agent_id}
              fallback={metadata.nav_mark ?? metadata.display_name.slice(0, 1)}
              size={50}
            />
          </span>
          <div>
            <span className="eyebrow">AGENT ROUTE</span>
            {embedded ? <h2>{metadata.display_name}</h2> : <h1>{metadata.display_name}</h1>}
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
            onboardingTarget="agent-installation"
            onSelect={(path) => {
              setSelectedPath(path);
              onInstallationSelected?.();
            }}
          />
          <button
            className={`btn agent-primary-action ${managed ? "" : "primary"}`}
            type="button"
            data-onboarding-target={!managed ? "agent-connect" : undefined}
            disabled={busy || !canOperate}
            onClick={() => void (managed ? restoreOfficial() : applyConnection())}
            title={managed
              ? copy(
                "Strip the fields Token Station injected and return the Agent to its official default configuration, then clear the management record.",
                "剥掉 Token Station 注入的字段，让 Agent 回到官方默认配置，并清除接管记录。",
              )
              : undefined}
          >
            {busy
              ? copy("Working…", "处理中…")
              : managed
                ? copy("Restore official configuration & disconnect", "恢复官方配置并断开")
                : copy("Connect", "一键接入")}
          </button>
        </div>
      </header>

      {connectDiff && (() => {
        const changes = connectDiff.changes ?? [];
        const keyChanges = changes.filter((change) => KEY_CHANGE_HINT.test(change.path.segments.join(".")));
        const shown = keyChanges.length ? keyChanges : changes.slice(0, 3);
        const rest = changes.length - shown.length;
        return (
          <section className="panel connect-diff-card">
            <div className="connect-diff-head">
              <div>
                <span className="eyebrow">{copy("FIRST CONNECTION · WHAT CHANGED", "首次接入 · 我们动了什么")}</span>
                <h2>{copy(
                  `${metadata.display_name.replace(" Agent", "")} is connected. Here is exactly what changed.`,
                  `已接入 ${metadata.display_name.replace(" Agent", "")}，改动如实告知`,
                )}</h2>
              </div>
              <button className="btn" type="button" onClick={dismissConnectDiff}>
                {copy("Got it", "知道了")}
              </button>
            </div>
            <p className="connect-diff-intro">
              {copy("Only the ", "只改了让请求经本地网关必需的")}
              <strong>{copy("required routing fields", "这几个关键字段")}</strong>
              {copy(
                " were changed. Your original configuration was backed up and can be restored when disconnecting:",
                "，你的原配置已自动备份，断开时一键还原：",
              )}
            </p>
            <ul className="connect-diff-list">
              {shown.map((change, index) => (
                <li key={index}>
                  <code>{change.path.segments.join(".")}</code>
                  <span className="connect-diff-summary">{change.summary}</span>
                </li>
              ))}
            </ul>
            {rest > 0 && <p className="connect-diff-rest">{copy(
              `${rest} additional supporting settings were adjusted.`,
              `另有 ${rest} 项辅助开关调整。`,
            )}</p>}
            <p className="connect-diff-foot">{copy(
              "This notice appears only on the first connection.",
              "此提示仅在首次接入时出现一次，之后不再打扰。",
            )}</p>
          </section>
        );
      })()}


      {!serveRunning && !managed && <div className="inline-note">{copy(
        "Start the proxy before connecting an Agent. Routing can still be configured now.",
        "请先启动代理，再接入 Agent。路由仍可先行配置。",
      )}</div>}
      {notice && <div className="banner ok">{notice}</div>}
      {error && <div className="banner err">{error}</div>}

      <RoutingModeSelector
        value={route.routing_mode}
        disabled={busy}
        agent
        onValueChange={onSetRoutingMode}
      />

      {route.routing_mode === "quota_first" ? (
        <QuotaPriorityPanel
          providers={providers}
          accounts={quotaAccounts}
          busy={busy}
          applying={applying}
          onSave={onSaveQuota}
          onViewUsage={onViewQuotaUsage}
          onSavePlan={onSaveQuotaPlan}
        />
      ) : (
      <section className="panel route-panel">
        <div className="panel-head split-heading">
          <div>
            <span className="eyebrow">ROUTING PROFILE</span>
            <h2>{copy("Three-tier routing", "三档路由")}</h2>
            <p className="sub">{copy(
              "Provider and model selection matches Home. This only determines whether the Agent uses a custom configuration.",
              "供应商和模型选择与主页完全一致；只决定此 Agent 是否使用独立配置。",
            )}</p>
          </div>
          <div className="mode-switch" role="radiogroup" aria-label={copy("Agent routing mode", "Agent 路由模式")}>
            <button type="button" role="radio" aria-checked={route.mode === "inherit"} className={route.mode === "inherit" ? "active" : ""} disabled={busy} onClick={() => void switchMode("inherit")}>{copy("Follow Home", "跟随主页")}</button>
            <button type="button" role="radio" aria-checked={route.mode === "custom"} className={route.mode === "custom" ? "active" : ""} disabled={busy} onClick={() => void switchMode("custom")}>{copy("Custom routing", "独立路由")}</button>
            <button type="button" role="radio" aria-checked={route.mode === "profile"} className={route.mode === "profile" ? "active" : ""} disabled={busy} onClick={() => void mountProfile()}>{copy("Use profile", "挂载策略组")}</button>
          </div>
        </div>

        {route.mode === "profile" && (
          <label className="profile-mount-select">
            <span>{copy("Current profile", "当前策略组")}</span>
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
              <button className="btn primary" type="button" disabled={busy || Boolean(route.config_error)} onClick={() => void saveRoute()}>{copy("Save & restart", "保存并重启")}</button>
              <button className="btn" type="button" disabled={busy} onClick={() => void restoreHome()}>{copy("Restore home routing", "恢复主页路由")}</button>
            </>
          ) : route.mode === "profile" ? (
            <span className="inherit-note">{copy(
              `This Agent uses routing profile “${route.profile}”. Manage profiles on Home, then save and apply.`,
              `该 Agent 使用策略组「${route.profile}」。在主页管理策略组，保存并应用后生效。`,
            )}</span>
          ) : (
            <>
              <span className="inherit-note">{copy(
                "This Agent automatically uses the latest three-tier configuration from Home.",
                "主页路由更新后，此 Agent 会自动使用最新三档配置。",
              )}</span>
              <button
                className="btn primary"
                type="button"
                disabled={busy}
                onClick={() => void applyHomeRoute()}
                title={copy(
                  "Apply the current Home three-tier routing to this Agent now.",
                  "立即将主页当前三档路由应用到此 Agent。",
                )}
              >
                {busy ? copy("Working…", "处理中…") : copy("Apply", "应用")}
              </button>
            </>
          )}
          {route.config_error && <span className="foot-hint error-text">{humanizeAppError(route.config_error)}</span>}
        </footer>
      </section>
      )}
    </div>
  );
}
