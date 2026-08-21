import { useEffect, useMemo, useState } from "react";
import { Check as CheckIcon, Copy as CopyIcon, Route as RouteIcon } from "lucide-react";
import {
  applyAgentPlan,
  configureCursorProvider,
  ensureServeRunning,
  forceForgetAgent,
  getCursorProviderStatus,
  mountAgentProfile,
  planAgentConnection,
  saveAgentRoutes,
  restartAgentRoute,
  restoreCursorProvider,
  setAgentRouteMode,
  setAgentTier,
  type AgentInstallationView,
  type ConfigPlanView,
  type CursorProviderStatusView,
  type AgentRouteView,
  type AgentUiMetadataView,
  type AgentView,
  type ProviderView,
  type QuotaAccount,
  type RoutingMode,
  type StateView,
  type TierSlot,
} from "../api";
import TierRouteEditor from "../components/TierRouteEditor";
import InstallationPicker from "../components/InstallationPicker";
import QuotaPriorityPanel from "../components/QuotaPriorityPanel";
import RoutingModeSelector from "../components/RoutingModeSelector";
import DirectRoutePanel from "../components/DirectRoutePanel";
import { useErrorToast } from "../components/ErrorToast";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "../components/ui/select";
import { AgentIcon } from "../brandIcons";
import { useLocalizedCopy, type Language } from "../components/LanguageProvider";
import { humanizeAppError } from "../errors";
import { Button } from "../components/ui/button";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "../components/ui/tooltip";

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
  onRefreshAgents: () => void | Promise<void>;
  onConnectInFlightChange?: (inFlight: boolean) => void;
  onSaveQuota: (accounts: QuotaAccount[]) => void;
  onSaveQuotaPlan: (
    upstream: string,
    lenMs: number,
    limit: number,
    unit: "tokens" | "requests",
  ) => void;
  onViewQuotaUsage: () => void;
  onSetRoutingMode?: (mode: RoutingMode) => void;
  onApplyDirect?: (upstream: string, model: string) => void | Promise<boolean>;
  onDeleteProfile?: (name: string) => void | Promise<boolean>;
  onInstallationSelected?: () => void;
  selectedInstallationPath?: string;
  onInstallationPathChange?: (path: string) => void;
  embedded?: boolean;
  pageMode?: "combined" | "connection" | "routing";
}

/** Per-Agent key recording that connection changes were shown; localStorage makes it appear only once. */
const diffShownKey = (agentId: string) => `ts:agent-connect-diff-shown:${agentId}`;

/** Managed fields that best explain where data flows and therefore matter most to users. */
const KEY_CHANGE_HINT = /url|base|token|key|auth|endpoint|host|proxy/i;

function errorText(error: unknown) {
  return humanizeAppError(error);
}

export function compactDiscoveryPath(path: string): string {
  const hasWindowsDrive = /^[A-Za-z]:[\\/]/.test(path);
  const isWindowsUnc = path.startsWith("\\\\");
  const isPosixAbsolute = path.startsWith("/");
  if (!hasWindowsDrive && !isWindowsUnc && !isPosixAbsolute) return path;

  const separator = hasWindowsDrive || isWindowsUnc ? "\\" : "/";
  const segments = path.split(/[\\/]+/).filter(Boolean);
  const leadingSegments = hasWindowsDrive ? 3 : 2;
  if (segments.length <= leadingSegments + 4) return path;

  const start = segments.slice(0, leadingSegments).join(separator);
  const end = segments.slice(-4).join(separator);
  const prefix = hasWindowsDrive ? "" : isWindowsUnc ? "\\\\" : "/";
  return `${prefix}${start}${separator}…${separator}${end}`;
}

function statusCopy(
  metadata: AgentUiMetadataView,
  agent: AgentView | undefined,
  installation: AgentInstallationView | undefined,
  copy: (english: string, simplifiedChinese: string) => string,
  language: Language,
) {
  const usesCursorDatabaseIntegration = metadata.agent_id === "cursor"
    && installation?.compatibility.reason_code === "CONNECTOR_BINDING_NOT_UNIQUE";
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
  if (installation
    && installation.compatibility.status !== "DETECTED_VERIFIED"
    && !isExactMultiInstallSelection(agent, installation)
    && !usesCursorDatabaseIntegration) {
    return {
      tone: "danger",
      label: copy("Unavailable", "暂不可接入"),
      detail: humanizeAppError({
        code: installation.compatibility.reason_code,
        message: installation.compatibility.message,
      }, language),
    };
  }
  if (installation?.connection_issue) {
    return {
      tone: "danger",
      label: installation.connection_issue.code === "agent_runtime_transition"
        ? copy("Proxy transitioning", "代理切换中")
        : copy("Route incomplete", "路由待完善"),
      detail: humanizeAppError(installation.connection_issue, language),
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
        "运行中的 Cursor 不会被强制关闭。请退出 Cursor 后再点一键接入并启动。",
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
  onRefreshAgents,
  onConnectInFlightChange,
  onSaveQuota,
  onSaveQuotaPlan,
  onViewQuotaUsage,
  onSetRoutingMode = () => {},
  onApplyDirect = () => {},
  onDeleteProfile = () => {},
  onInstallationSelected,
  selectedInstallationPath,
  onInstallationPathChange,
  embedded = false,
  pageMode = "combined",
}: AgentRoutePageProps) {
  const { copy, language } = useLocalizedCopy();
  const { showError, showSuccess } = useErrorToast();
  const [localSelectedPath, setLocalSelectedPath] = useState("");
  const selectedPath = selectedInstallationPath ?? localSelectedPath;
  const [busy, setBusy] = useState(false);
  const [cursorStatus, setCursorStatus] = useState<CursorProviderStatusView | null>(null);
  // Show configuration changes after the first connection, then persist dismissal in localStorage.
  const [connectDiff, setConnectDiff] = useState<ConfigPlanView | null>(null);
  const [copiedDiscoveryPath, setCopiedDiscoveryPath] = useState<{ path: string } | null>(null);
  const dismissConnectDiff = () => setConnectDiff(null);

  useEffect(() => {
    if (selectedInstallationPath !== undefined) return;
    const paths = agent?.installations.map((item) => item.discovery.canonical_path) ?? [];
    setLocalSelectedPath((current) => paths.includes(current) ? current : paths.length === 1 ? paths[0] : "");
  }, [agent, selectedInstallationPath]);

  useEffect(() => {
    if (metadata.agent_id !== "cursor") {
      setCursorStatus(null);
      return;
    }
    let cancelled = false;
    void getCursorProviderStatus()
      .then((next) => {
        if (!cancelled) setCursorStatus(next);
      })
      .catch((caught) => {
        if (!cancelled) showError(errorText(caught), "cursor-provider-status");
      });
    return () => {
      cancelled = true;
    };
  }, [metadata.agent_id, showError]);

  useEffect(() => {
    if (copiedDiscoveryPath === null) return undefined;
    const copiedState = copiedDiscoveryPath;
    const timer = window.setTimeout(() => {
      setCopiedDiscoveryPath((current) => (current === copiedState ? null : current));
    }, 1_600);
    return () => window.clearTimeout(timer);
  }, [copiedDiscoveryPath]);

  const installation = useMemo(
    () => agent?.installations.find((item) => item.discovery.canonical_path === selectedPath),
    [agent, selectedPath],
  );

  const discoveredStatus = statusCopy(metadata, agent, installation, copy, language);
  const status = metadata.agent_id === "cursor" && cursorStatus?.state === "connected"
    ? {
      tone: "success" as const,
      label: copy("Connected", "已接入"),
      detail: cursorStatus.message ?? copy(
        "Requests are routed through Token Station.",
        "请求已通过 Token Station。",
      ),
    }
    : metadata.agent_id === "cursor" && cursorStatus?.state === "repair_required"
      ? {
        tone: "danger" as const,
        label: copy("Repair needed", "需修复"),
        detail: cursorStatus.message ?? copy(
          "The previous Cursor tunnel is no longer active. Restore and reconnect.",
          "上次 Cursor 隧道已失效，请恢复后重新接入。",
        ),
      }
      : discoveredStatus;
  const cursorRepairRequired = metadata.agent_id === "cursor"
    && cursorStatus?.state === "repair_required";
  const managed = metadata.agent_id === "cursor"
    ? cursorStatus?.state === "connected"
    : installation?.managed ?? false;
  const canConnect = Boolean(
    installation
      && installation.adapter_ready !== false
      && !installation.connection_issue
      && installation.compatibility.reason_code !== "READ_ONLY_PREFLIGHT_FAILED"
      && (metadata.agent_id === "cursor"
        || ["DETECTED_VERIFIED", "CONNECTED"].includes(installation.compatibility.status)
        || isExactMultiInstallSelection(agent, installation)),
  );
  const canOperate = managed ? Boolean(installation) : canConnect;
  const connectionTarget = installation?.discovery.config_candidates[0]
    ?? metadata.connector_capabilities?.[0]?.config_path_template
    ?? copy("Resolved during connection", "接入时确定");
  const ownedFields = metadata.connector_capabilities?.[0]?.owned_fields ?? [];
  const routeNeedsAttention = Boolean(route.config_error || !route.direct_target?.model);
  const discoveredPath = installation?.discovery.canonical_path;
  const discoveredPathCopied = copiedDiscoveryPath?.path === discoveredPath;

  const copyDiscoveredPath = async () => {
    const pathToCopy = discoveredPath;
    if (!pathToCopy) return;
    try {
      await navigator.clipboard.writeText(pathToCopy);
      setCopiedDiscoveryPath({ path: pathToCopy });
    } catch {
      showError(
        copy(
          "Unable to copy the discovered path. Check system clipboard permissions and try again.",
          "无法复制发现路径，请检查系统剪贴板权限后重试。",
        ),
        `agent-discovery-path-copy:${metadata.agent_id}`,
      );
    }
  };

  const runState = async (action: () => Promise<StateView>, message?: string) => {
    if (busy) return;
    setBusy(true);
    try {
      onStateChange(await action());
      if (message) showSuccess(message, `agent-route-action:${metadata.agent_id}`);
    } catch (caught) {
      showError(errorText(caught), `agent-route-action:${metadata.agent_id}`);
    } finally {
      setBusy(false);
    }
  };

  // Connect immediately after planning without a redundant confirmation wait.
  // The post-connection diff card still appears once for transparency.
  const applyConnection = async () => {
    if (!installation || !canOperate || busy) return;
    onConnectInFlightChange?.(true);
    setBusy(true);
    try {
      onStateChange(await ensureServeRunning());
      if (metadata.agent_id === "cursor") {
        const next = await configureCursorProvider();
        setCursorStatus(next);
        showSuccess(
          next.message ?? copy("Cursor connected.", "Cursor 已接入"),
          `agent-connect:${metadata.agent_id}`,
        );
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
      if (plan.changes?.length || plan.human_diff) {
        let shouldShowDiff = true;
        try {
          shouldShowDiff = !localStorage.getItem(diffShownKey(metadata.agent_id));
          if (shouldShowDiff) {
            localStorage.setItem(diffShownKey(metadata.agent_id), String(Date.now()));
          }
        } catch {
          showError(
            copy(
              "The Agent connected, but Token Station could not remember whether the first-connection changes were shown.",
              "Agent 已接入，但无法保存首次接入差异提示状态。",
            ),
            `agent-connect-diff-storage:${metadata.agent_id}`,
          );
        }
        if (shouldShowDiff) setConnectDiff(plan);
      }
      showSuccess(
        copy("Agent connected.", "Agent 已接入"),
        `agent-connect:${metadata.agent_id}`,
      );
    } catch (caught) {
      showError(errorText(caught), `agent-connect:${metadata.agent_id}`);
    } finally {
      onConnectInFlightChange?.(false);
      setBusy(false);
      try {
        await onRefreshAgents();
      } catch (caught) {
        showError(errorText(caught), `agent-refresh:${metadata.agent_id}`);
      }
    }
  };

  // Restore official configuration and disconnect by removing TS-managed fields
  // according to ownership records, returning the Agent to official defaults,
  // then clearing ownership. This deterministic path replaces the old force-
  // disconnect fallback and does not depend on encrypted snapshots or a master key.
  const restoreOfficial = async () => {
    if (!installation || busy) return;
    setBusy(true);
    let restored = false;
    try {
      if (metadata.agent_id === "cursor") {
        const next = await restoreCursorProvider();
        setCursorStatus(next);
        showSuccess(
          next.message ?? copy(
            "Restored the official Cursor configuration and disconnected.",
            "已恢复 Cursor 官方配置并断开。",
          ),
          `agent-restore-official:${metadata.agent_id}`,
        );
        restored = true;
        return;
      }
      await forceForgetAgent(metadata.agent_id, installation.discovery.canonical_path);
      restored = true;
      showSuccess(
        copy(
          "Restored the official configuration and disconnected.",
          "已恢复官方配置并断开。",
        ),
        `agent-restore-official:${metadata.agent_id}`,
      );
    } catch (caught) {
      showError(errorText(caught), `agent-restore-official:${metadata.agent_id}`);
    } finally {
      if (restored) {
        try {
          await onRefreshAgents();
        } catch (caught) {
          showError(errorText(caught), `agent-refresh:${metadata.agent_id}`);
        }
      }
      setBusy(false);
    }
  };

  const switchMode = async (mode: "inherit" | "custom") => {
    await runState(() => setAgentRouteMode(metadata.agent_id, mode));
  };

  const mountProfile = async (profile = route.profile ?? profiles[0]) => {
    if (!profile) {
      showError(
        copy(
          "No routing profile is available. Save the home routing configuration as a profile first.",
          "还没有可挂载的策略组，请先在主页将三档路由另存为策略组。",
        ),
        `agent-profile:${metadata.agent_id}`,
      );
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

  const removeCurrentProfile = async () => {
    if (!route.profile || busy) return;
    setBusy(true);
    try {
      await onDeleteProfile(route.profile);
    } finally {
      setBusy(false);
    }
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
    try {
      await setAgentRouteMode(metadata.agent_id, "inherit");
      const next = await saveAgentRoutes();
      onStateChange(next);
      showSuccess(
        serveRunning
          ? copy("Restored home routing · Restart the proxy to apply", "已恢复跟随主页 · 重启代理后生效")
          : copy("Restored home routing", "已恢复跟随主页"),
        `agent-restore-home:${metadata.agent_id}`,
      );
    } catch (caught) {
      showError(errorText(caught), `agent-restore-home:${metadata.agent_id}`);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className={`page-stack agent-route-page ${embedded ? "agent-route-embedded" : ""}`}>
      {pageMode === "routing" && (
        <header className="agent-route-routing-heading panel">
          <div className="agent-route-heading-identity">
            <span className="agent-large-mark" aria-hidden="true">
              <AgentIcon
                id={metadata.agent_id}
                fallback={metadata.nav_mark ?? metadata.display_name.slice(0, 1)}
                size={42}
              />
            </span>
            {embedded ? <h2>{metadata.display_name}</h2> : <h1>{metadata.display_name}</h1>}
          </div>
          <span className="status-chip neutral">{route.mode === "inherit" ? copy("Follows global", "跟随全局") : copy("Independent", "独立路由")}</span>
        </header>
      )}
      {pageMode !== "routing" && (
        <>
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
              if (onInstallationPathChange) {
                onInstallationPathChange(path);
              } else {
                setLocalSelectedPath(path);
              }
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
                : cursorRepairRequired
                  ? copy("Reconnect & launch", "重新接入并启动")
                  : metadata.agent_id === "cursor"
                    ? copy("Connect & launch", "一键接入并启动")
                    : copy("Connect", "一键接入")}
          </button>
          {pageMode !== "connection" && cursorRepairRequired ? (
            <button
              className="btn agent-secondary-action"
              type="button"
              disabled={busy || !installation}
              onClick={() => void restoreOfficial()}
              title={copy(
                "Restore the original Cursor configuration and clear the stale management record.",
                "恢复 Cursor 原配置，并清除失效的接管记录。",
              )}
            >
              {copy("Restore official configuration & disconnect", "恢复官方配置并断开")}
            </button>
          ) : null}
        </div>
      </header>

      <section className="panel agent-connection-detail" aria-label={copy("Agent connection details", "Agent 接入详情")}>
        <dl className="agent-connection-facts">
          <div className="agent-discovered-path-fact">
            <dt>{copy("Discovered path", "发现路径")}</dt>
            <dd>
              {discoveredPath ? (
                <>
                  <TooltipProvider>
                    <Tooltip>
                      <TooltipTrigger asChild>
                        <code className="agent-discovered-path" tabIndex={0}>
                          {compactDiscoveryPath(discoveredPath)}
                        </code>
                      </TooltipTrigger>
                      <TooltipContent className="agent-discovered-path-tooltip">
                        <code>{discoveredPath}</code>
                      </TooltipContent>
                    </Tooltip>
                  </TooltipProvider>
                  <Button
                    className="agent-discovered-path-copy"
                    variant="ghost"
                    size="icon-sm"
                    type="button"
                    aria-label={discoveredPathCopied
                      ? copy("Discovered path copied", "发现路径已复制")
                      : copy("Copy discovered path", "复制发现路径")}
                    title={discoveredPathCopied
                      ? copy("Copied", "已复制")
                      : copy("Copy full path", "复制完整路径")}
                    onClick={() => void copyDiscoveredPath()}
                  >
                    {discoveredPathCopied
                      ? <CheckIcon data-icon="inline-start" aria-hidden="true" />
                      : <CopyIcon data-icon="inline-start" aria-hidden="true" />}
                  </Button>
                </>
              ) : (
                <code>{(agent?.installations.length ?? 0) > 1
                  ? copy("Choose a version above", "请先选择版本")
                  : copy("Not found", "未发现")}</code>
              )}
            </dd>
          </div>
          <div><dt>{copy("Version", "版本")}</dt><dd><strong>{installation?.discovery.version_normalized ?? installation?.discovery.version_raw ?? ((agent?.installations.length ?? 0) > 1 ? copy("Not selected", "未选择") : copy("Unknown", "未知"))}</strong></dd></div>
        </dl>
        <div className="agent-connection-change">
          <div>
            <h3>{copy("Connection changes", "接入将修改")}</h3>
            <p>{copy("Token Station backs up the original file before it writes managed routing fields.", "Token Station 写入受管路由字段前会备份原文件。")}</p>
          </div>
          <div className="agent-connection-file">
            <code>{connectionTarget}</code>
            <small>{ownedFields.length > 0
              ? copy(`Managed fields: ${ownedFields.join(", ")}`, `受管字段：${ownedFields.join("、")}`)
              : copy("The exact non-sensitive changes appear after the connection plan is created.", "生成接入计划后会显示确切的非敏感改动。")}</small>
          </div>
        </div>
        <div className={`agent-default-route-state ${routeNeedsAttention ? "warning" : ""}`}>
          <span aria-hidden="true">{routeNeedsAttention ? "!" : <RouteIcon />}</span>
          <div>
            <strong>{copy("Route preview after connection", "接入后的路由预览")}</strong>
            <code>{route.direct_target?.upstream && route.direct_target.model
              ? `${route.direct_target.upstream} / ${route.direct_target.model}`
              : copy("Complete global routing before connection", "请先完成全局路由")}</code>
          </div>
        </div>
      </section>

      {connectDiff && (() => {
        const changes = connectDiff.changes ?? [];
        const keyChanges = changes.filter((change) => KEY_CHANGE_HINT.test(change.path.segments.join(".")));
        const shown = keyChanges.length ? keyChanges : changes.slice(0, 3);
        const rest = changes.length - shown.length;
        return (
          <section className="panel connect-diff-card">
            <div className="connect-diff-head">
              <div>
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
        </>
      )}
      {pageMode !== "connection" && (
        <>
      <RoutingModeSelector
        value={route.routing_mode}
        disabled={busy}
        agent
        onValueChange={onSetRoutingMode}
      />

      {route.routing_mode === "direct" ? (
        <DirectRoutePanel
          providers={providers}
          target={route.direct_target ?? null}
          busy={busy}
          applying={applying}
          agent
          onApply={(upstream, model) => {
            void onApplyDirect(upstream, model);
          }}
        />
      ) : route.routing_mode === "quota_first" ? (
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
            <h2>{copy("Three-tier routing", "三档路由")}</h2>
            <p className="sub">{copy(
              "Provider and model selection matches Home. This only determines whether the Agent uses a custom configuration.",
              "供应商和模型选择与主页一致，只决定当前客户端是否使用独立配置。",
            )}</p>
          </div>
          <div className="agent-tier-heading-actions">
            <div className="mode-switch" role="radiogroup" aria-label={copy("Agent routing mode", "Agent 路由模式")}>
              <button type="button" role="radio" aria-checked={route.mode === "inherit"} className={route.mode === "inherit" ? "active" : ""} disabled={busy} onClick={() => void switchMode("inherit")}>{copy("Follow Home", "跟随主页")}</button>
              <button type="button" role="radio" aria-checked={route.mode === "custom"} className={route.mode === "custom" ? "active" : ""} disabled={busy} onClick={() => void switchMode("custom")}>{copy("Custom tiers", "自定义三档")}</button>
              <button type="button" role="radio" aria-checked={route.mode === "profile"} className={route.mode === "profile" ? "active" : ""} disabled={busy} onClick={() => void mountProfile()}>{copy("Use profile", "挂载策略组")}</button>
            </div>
            {route.mode === "custom" ? (
              <div className="agent-tier-apply-actions">
                <button className="btn" type="button" disabled={busy} onClick={() => void restoreHome()}>{copy("Restore home routing", "恢复主页路由")}</button>
                <button className="btn primary" type="button" disabled={busy || Boolean(route.config_error)} onClick={() => void saveRoute()}>{copy("Save & restart", "保存并重启")}</button>
              </div>
            ) : route.mode === "inherit" ? (
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
            ) : null}
          </div>
        </div>

        {route.mode === "profile" && (
          <div className="profile-mount-select">
            <span>{copy("Current profile", "当前策略组")}</span>
            <div className="profile-mount-controls">
            <Select
              value={route.profile ?? ""}
              disabled={busy}
              onValueChange={(profile) => void mountProfile(profile)}
            >
              <SelectTrigger aria-label={copy("Current profile", "当前策略组")}>
                <SelectValue placeholder={copy("Choose a profile", "选择策略组")} />
              </SelectTrigger>
              <SelectContent>
                <SelectGroup>
                  {profiles.map((profile) => <SelectItem key={profile} value={profile}>{profile}</SelectItem>)}
                </SelectGroup>
              </SelectContent>
            </Select>
            <button
              className="btn danger"
              type="button"
              disabled={busy || !route.profile}
              aria-label={copy(
                `Delete profile ${route.profile ?? ""}`,
                `删除策略组 ${route.profile ?? ""}`,
              )}
              onClick={() => void removeCurrentProfile()}
            >
              {copy("Delete", "删除")}
            </button>
            </div>
          </div>
        )}

        <TierRouteEditor
          tiers={route.tiers}
          providers={providers}
          disabled={busy}
          readOnly={route.mode !== "custom"}
          onTierChange={(slot: TierSlot, upstream, model) => runState(() => setAgentTier(metadata.agent_id, slot, upstream, model))}
        />

        <footer className="panel-foot route-actions">
          {route.mode === "profile" ? (
            <span className="inherit-note">{copy(
              `This Agent uses routing profile “${route.profile}”. Manage profiles on Home, then save and apply.`,
              `该 Agent 使用策略组「${route.profile}」。在主页管理策略组，保存并应用后生效。`,
            )}</span>
          ) : (
            route.mode === "inherit" ? (
              <span className="inherit-note">{copy(
                "This Agent automatically uses the latest three-tier configuration from Home.",
                "主页路由更新后，此 Agent 会自动使用最新三档配置。",
              )}</span>
            ) : null
          )}
          {route.config_error && <span className="foot-hint error-text">{humanizeAppError(route.config_error)}</span>}
        </footer>
      </section>
      )}
        </>
      )}
    </div>
  );
}
