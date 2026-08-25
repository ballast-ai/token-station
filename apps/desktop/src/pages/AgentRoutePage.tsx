import { useEffect, useMemo, useState } from "react";
import {
  Check as CheckIcon,
  Copy as CopyIcon,
  Globe2,
  Route as RouteIcon,
  SlidersHorizontal,
} from "lucide-react";
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
import { useLocalizedCopy, type Language, type LocalizedCopy } from "../components/LanguageProvider";
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
  copy: LocalizedCopy,
  language: Language,
) {
  const usesCursorDatabaseIntegration = metadata.agent_id === "cursor"
    && installation?.compatibility.reason_code === "CONNECTOR_BINDING_NOT_UNIQUE";
  if (!agent || agent.installations.length === 0) {
    return {
      tone: "idle",
      label: copy("Not found", "未发现", "未找到", "見つかりません"),
      detail: copy("No manageable installation was found on this device.", "没有在本机发现可管理的安装。", "在本機未發現可管理的安裝。", "このデバイスには管理可能なインストールが見つかりません。"),
    };
  }
  if (agent.installations.length > 1 && !installation) {
    return {
      tone: "idle",
      label: copy("Select one", "待选择", "請選擇一個", "1つ選択してください"),
      detail: copy(
        "Multiple installations were detected. Select the exact path to manage.",
        "检测到多份安装，请先选择要接管的精确路径。", "檢測到多份安裝，請先選擇要接管的精確路徑。", "複数のインストールが検出されました。管理するための正確なパスを選択してください。"
      ),
    };
  }
  if (installation?.adapter_ready === false) {
    return {
      tone: "danger",
      label: copy("Adapter unavailable", "适配器未就绪", "介面卡不可用", "アダプターが利用できません"),
      detail: installation.managed
        ? copy(
            "The managed configuration still exists, but the running Gateway did not load its required inbound adapter. Requests cannot be served; restore the original configuration or repair the adapter and restart the proxy.",
            "接管配置仍存在，但当前 Gateway 未加载所需入站适配器，无法处理请求。请恢复原始配置，或修复适配器后重启代理。", "接管配置仍存在，但當前 Gateway 未載入所需入站介面卡，無法處理請求。請恢復原始配置，或修復介面卡後重啟代理。", "管理された設定はまだ存在しますが、現在の Gateway は必要なインバウンドアダプターをロードしていません。リクエストの処理が不可能です。元の設定に戻すか、アダプターを修復してプロキシを再起動してください。"
          )
        : copy(
            "The running Gateway did not load the required inbound adapter. No Agent configuration was changed.",
            "当前 Gateway 未加载所需入站适配器，暂不可接入；Agent 配置未被修改。", "當前 Gateway 未載入所需入站介面卡，暫不可接入；Agent 配置未被修改。", "現在の Gateway は必要なインバウンドアダプターをロードしていません。一時的に接続できません。Agent の設定は変更されていません。"
      ),
    };
  }
  if (installation?.compatibility.reason_code === "READ_ONLY_PREFLIGHT_FAILED") {
    return {
      tone: "danger",
      label: copy("Management state unavailable", "接管状态不可用", "接管狀態不可用", "管理状態が利用できません"),
      detail: copy(
        "The Agent remains visible in read-only mode, but Token Station cannot verify its management records. Connect and recovery actions are disabled.",
        "Agent 仍可只读显示，但 Token Station 无法验证接管记录，接入和恢复操作已禁用。", "Agent 仍可只讀顯示，但 Token Station 無法驗證接管記錄，接入和恢復操作已停用。", "Agent は読み取り専用の表示のままですが、Token Station は管理記録を検証できません。接続および復旧操作は無効です。"
      ),
    };
  }
  if (installation
    && installation.compatibility.status !== "DETECTED_VERIFIED"
    && !isExactMultiInstallSelection(agent, installation)
    && !usesCursorDatabaseIntegration) {
    return {
      tone: "danger",
      label: copy("Unavailable", "暂不可接入", "不可用", "利用不可"),
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
        ? copy("Proxy transitioning", "代理切换中", "代理切換中", "プロキシの切り替え中")
        : copy("Route incomplete", "路由待完善", "路由待完善", "ルーティングが未完成"),
      detail: humanizeAppError(installation.connection_issue, language),
    };
  }
  if (installation?.connected) {
    return {
      tone: "success",
      label: copy("Connected", "已接入", "已接入", "接続済み"),
      detail: copy("Requests are routed through Token Station.", "请求已通过 Token Station。", "請求已通過 Token Station。", "リクエストは Token Station を通じてルーティングされます。"),
    };
  }
  if (metadata.agent_id === "cursor" && installation) {
    return {
      tone: "ready",
      label: copy("Ready", "可接入", "可接入", "接続可能"),
      detail: copy(
        "Cursor settings will be backed up and configured automatically.",
        "运行中的 Cursor 不会被强制关闭。请退出 Cursor 后再点一键接入并启动。", "執行中的 Cursor 不會被強制關閉。請退出 Cursor 後再點一鍵接入並啟動。", "実行中の Cursor は強制的に終了しません。Cursor を終了した後、ワンクリックで接続して起動してください。"
      ),
    };
  }
  if (installation?.managed) {
    return {
      tone: "danger",
      label: copy("Repair needed", "需修复", "需修復", "修復が必要"),
      detail: copy(
        "A management record exists, but the runtime state does not match. Restore the original configuration before reconnecting.",
        "已有接管记录，但当前运行态不一致。请先恢复原始配置，再重新接入。", "已有接管記錄，但當前執行態不一致。請先恢復原始配置，再重新接入。", "既存の管理レコードが存在しますが、現在の実行状態が一致しません。再接続する前に元の設定に戻してください。"
      ),
    };
  }
  if (isExactMultiInstallSelection(agent, installation)) {
    return {
      tone: "ready",
      label: copy("Ready", "可接入", "可接入", "接続可能"),
      detail: copy("The exact installation is selected and ready to connect.", "已选择精确安装，可以一键接入。", "已選擇精確安裝，可以一鍵接入。", "正確なインストールが選択され、ワンクリックで接続可能です。"),
    };
  }
  return {
    tone: "ready",
    label: copy("Ready", "可接入", "可接入", "接続可能"),
    detail: copy("A compatible installation was found and is ready to connect.", "已发现兼容安装，可以一键接入。", "已發現相容安裝，可以一鍵接入。", "互換性のあるインストールが見つかり、ワンクリックで接続可能です。"),
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
  const followsGlobal = route.inherits_global === true;
  const [independentEditorOpen, setIndependentEditorOpen] = useState(!followsGlobal);
  const dismissConnectDiff = () => setConnectDiff(null);

  useEffect(() => {
    setIndependentEditorOpen(!followsGlobal);
  }, [followsGlobal, metadata.agent_id]);

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
      label: copy("Connected", "已接入", "已接入", "接続済み"),
      detail: cursorStatus.message ?? copy(
        "Requests are routed through Token Station.",
        "请求已通过 Token Station。", "請求已通過 Token Station。", "リクエストは Token Station を通じてルーティングされます。"
      ),
    }
    : metadata.agent_id === "cursor" && cursorStatus?.state === "repair_required"
      ? {
        tone: "danger" as const,
        label: copy("Repair needed", "需修复", "需修復", "修復が必要"),
        detail: cursorStatus.message ?? copy(
          "The previous Cursor tunnel is no longer active. Restore and reconnect.",
          "上次 Cursor 隧道已失效，请恢复后重新接入。", "上次 Cursor 隧道已失效，請恢復後重新接入。", "以前の Cursor ツーリングが無効化されています。復元してから再接続してください。"
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
    ?? copy("Resolved during connection", "接入时确定", "接入時確定", "接続時に解決");
  const ownedFields = metadata.connector_capabilities?.[0]?.owned_fields ?? [];
  const routeNeedsAttention = Boolean(route.config_error || !route.direct_target?.model);
  const discoveredPath = installation?.discovery.canonical_path;
  const discoveredPathCopied = copiedDiscoveryPath?.path === discoveredPath;
  const inheritedStrategyName = route.routing_mode === "direct"
    ? copy("Simple routing", "简单路由", "簡單路由", "シンプルルーティング")
    : route.routing_mode === "quota_first"
      ? copy("Quota first", "额度优先", "額度優先", "クォータ優先")
      : copy("Smart tiers", "智能分档", "智慧分檔", "スマート階層");
  const inheritedStrategyDetail = route.routing_mode === "direct"
    ? route.direct_target?.upstream && route.direct_target.model
      ? `${route.direct_target.upstream} / ${route.direct_target.model}`
      : copy("Uses the global target", "使用全局目标", "使用全域目標", "グローバルターゲットを使用")
    : route.routing_mode === "quota_first"
      ? copy("Uses the global quota queue", "使用全局额度队列", "使用全域額度佇列", "グローバルクォータキューを使用")
      : copy("Uses the global high, mid, and low tiers", "使用全局高、中、低三档", "使用全域高、中、低三檔", "グローバルの高・中・低階層を使用");

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
          "无法复制发现路径，请检查系统剪贴板权限后重试。", "無法複製發現路徑，請檢查系統剪貼簿許可權後重試。", "発見されたパスをコピーできません。システムクリップボードの権限を確認してから再度試してください。"
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
          next.message ?? copy("Cursor connected.", "Cursor 已接入", "Cursor 已接入", "Cursor が接続済み"),
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
              "Agent 已接入，但无法保存首次接入差异提示状态。", "Agent 已接入，但無法儲存首次接入差異提示狀態。", "Agent が接続されました。ただし、最初の接続時の差異の表示状態を保存できません。"
            ),
            `agent-connect-diff-storage:${metadata.agent_id}`,
          );
        }
        if (shouldShowDiff) setConnectDiff(plan);
      }
      showSuccess(
        copy("Agent connected.", "Agent 已接入", "Agent 已連線。", "Agent が接続されました。"),
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
            "已恢复 Cursor 官方配置并断开。", "已恢復 Cursor 官方設定並斷開。", "Cursor の公式設定を復元し、接続を解除しました。"
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
          "已恢复官方配置并断开。", "已恢復官方設定並斷開。", "公式設定を復元し、接続を解除しました。"
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
          "还没有可挂载的策略组，请先在主页将三档路由另存为策略组。", "還沒有可掛載的策略群組，請先在首頁將三檔路由另存為策略群組。", "ルーティングプロファイルがありません。まずホームルーティング設定をプロファイルとして保存してください。"
        ),
        `agent-profile:${metadata.agent_id}`,
      );
      return;
    }
    await runState(
      () => mountAgentProfile(metadata.agent_id, profile),
      copy(
        `Routing profile “${profile}” mounted · Save and apply to finish`,
        `已挂载策略组「${profile}」· 尚待保存并应用`, `已掛載策略群組「${profile}」· 尚待儲存並應用`, `プロファイル「${profile}」がマウントされました · 保存して適用してください`
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
      ? copy("Custom routing saved and restarted for this Agent", "独立路由已保存并对此 Agent 生效", "獨立路由已儲存並對此 Agent 生效", "カスタムルーティングが保存され、この Agent に適用されました")
      : copy("Custom routing saved", "独立路由已保存", "獨立路由已儲存", "カスタムルーティングが保存されました"),
  );

  // In Follow Home mode, apply the current home tiers to this Agent immediately.
  // Restarting an inherited route clears the Agent-specific route and hot-applies the home configuration.
  const applyHomeRoute = () => runState(
    () => restartAgentRoute(metadata.agent_id),
    serveRunning
      ? copy("Home routing applied to this Agent", "已将主页路由应用到此 Agent", "已將首頁路由應用到此 Agent", "ホームルーティングがこの Agent に適用されました")
      : copy("Home routing saved · applies when the proxy starts", "已保存 · 启动代理后生效", "已儲存 · 啟動代理後生效", "保存されました · プロキシを起動後に有効になります"),
  );

  const restoreHome = async () => {
    if (busy) return;
    setBusy(true);
    try {
      await setAgentRouteMode(metadata.agent_id, "inherit");
      const next = await saveAgentRoutes();
      onStateChange(next);
      setIndependentEditorOpen(false);
      showSuccess(
        serveRunning
          ? copy("Restored home routing · Restart the proxy to apply", "已恢复跟随主页 · 重启代理后生效", "已恢復隨跟首頁 · 重啟代理後生效", "ホームルーティングを復元しました · プロキシを再起動後に有効になります")
          : copy("Restored home routing", "已恢复跟随主页", "已恢復隨跟首頁", "ホームルーティングを復元しました"),
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
          <span className="status-chip neutral">{followsGlobal ? copy("Follows global", "跟随全局", "隨跟全域性", "グローバルに従う") : copy("Independent", "独立路由", "獨立路由", "独立ルーティング")}</span>
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
                "剥掉 Token Station 注入的字段，让 Agent 回到官方默认配置，并清除接管记录。", "剝掉 Token Station 注入的欄位，讓 Agent 回到官方預設配置，並清除接管記錄。", "Token Station が注入したフィールドを除去し、Agent を公式のデフォルト設定に戻し、管理記録を削除してください。"
              )
              : undefined}
          >
            {busy
              ? copy("Working…", "处理中…", "處理中…", "処理中…")
              : managed
                ? copy("Restore official configuration & disconnect", "恢复官方配置并断开", "恢復官方配置並斷開", "公式設定を復元し、接続を解除")
                : cursorRepairRequired
                  ? copy("Reconnect & launch", "重新接入并启动", "重新連線並啟動", "再接続して起動")
                  : metadata.agent_id === "cursor"
                    ? copy("Connect & launch", "一键接入并启动", "連線並啟動", "接続して起動")
                    : copy("Connect", "一键接入", "連線", "接続")}
          </button>
          {pageMode !== "connection" && cursorRepairRequired ? (
            <button
              className="btn agent-secondary-action"
              type="button"
              disabled={busy || !installation}
              onClick={() => void restoreOfficial()}
              title={copy(
                "Restore the original Cursor configuration and clear the stale management record.",
                "恢复 Cursor 原配置，并清除失效的接管记录。", "恢復 Cursor 原設定，並清除失效的接管記錄。", "Cursor の元の設定に戻し、無効な管理記録を削除します。"
              )}
            >
              {copy("Restore official configuration & disconnect", "恢复官方配置并断开", "恢復官方配置並斷開", "公式設定を復元し、接続を解除")}
            </button>
          ) : null}
        </div>
      </header>

      <section className="panel agent-connection-detail" aria-label={copy("Agent connection details", "Agent 接入详情", "Agent 連線詳情", "Agent 接続詳細")}>
        <dl className="agent-connection-facts">
          <div className="agent-discovered-path-fact">
            <dt>{copy("Discovered path", "发现路径", "發現路徑", "発見されたパス")}</dt>
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
                      ? copy("Discovered path copied", "发现路径已复制", "發現路徑已複製", "発見されたパスがコピーされました")
                      : copy("Copy discovered path", "复制发现路径", "複製發現路徑", "発見されたパスをコピー")}
                    title={discoveredPathCopied
                      ? copy("Copied", "已复制", "已複製", "コピーしました")
                      : copy("Copy full path", "复制完整路径", "複製完整路徑", "完全なパスをコピー")}
                    onClick={() => void copyDiscoveredPath()}
                  >
                    {discoveredPathCopied
                      ? <CheckIcon data-icon="inline-start" aria-hidden="true" />
                      : <CopyIcon data-icon="inline-start" aria-hidden="true" />}
                  </Button>
                </>
              ) : (
                <code>{(agent?.installations.length ?? 0) > 1
                  ? copy("Choose a version above", "请先选择版本", "請先選擇版本", "まずバージョンを選択してください")
                  : copy("Not found", "未发现", "未找到", "見つかりません")}</code>
              )}
            </dd>
          </div>
          <div><dt>{copy("Version", "版本", "版本", "バージョン")}</dt><dd><strong>{installation?.discovery.version_normalized ?? installation?.discovery.version_raw ?? ((agent?.installations.length ?? 0) > 1 ? copy("Not selected", "未选择", "未選擇", "選択されていません") : copy("Unknown", "未知", "未知", "不明"))}</strong></dd></div>
        </dl>
        <div className="agent-connection-change">
          <div>
            <h3>{copy("Connection changes", "接入将修改", "連線將修改", "接続により変更されます")}</h3>
            <p>{copy("Token Station backs up the original file before it writes managed routing fields.", "Token Station 写入受管路由字段前会备份原文件。", "Token Station 在寫入受管路由欄位前會備份原檔案。", "Token Station は受管ルーティングフィールドを書き込む前に元のファイルをバックアップします。")}</p>
          </div>
          <div className="agent-connection-file">
            <code>{connectionTarget}</code>
            <small>{ownedFields.length > 0
              ? copy(`Managed fields: ${ownedFields.join(", ")}`, `受管字段：${ownedFields.join("、")}`, `受管欄位：${ownedFields.join(", ")}`, `管理フィールド：${ownedFields.join(", ")}`)
              : copy("The exact non-sensitive changes appear after the connection plan is created.", "生成接入计划后会显示确切的非敏感改动。", "生成接入計劃後會顯示確切的非敏感修改。", "接続計画が作成されると、正確な非敏感な変更が表示されます。")}</small>
          </div>
        </div>
        <div className={`agent-default-route-state ${routeNeedsAttention ? "warning" : ""}`}>
          <span aria-hidden="true">{routeNeedsAttention ? "!" : <RouteIcon />}</span>
          <div>
            <strong>{copy("Route preview after connection", "接入后的路由预览", "接入後路由預覽", "接続後のルーティングプレビュー")}</strong>
            <code>{route.direct_target?.upstream && route.direct_target.model
              ? `${route.direct_target.upstream} / ${route.direct_target.model}`
              : copy("Complete global routing before connection", "请先完成全局路由", "完成全域性路由後再進行連線", "グローバルルーティングを完了してから接続してください")}</code>
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
                  `已接入 ${metadata.display_name.replace(" Agent", "")}，改动如实告知`, `${metadata.display_name.replace(" Agent", "")} 連線成功。以下是變更內容`, `${metadata.display_name.replace(" Agent", "")} に接続しました。変更内容を以下に示します`
                )}</h2>
              </div>
              <button className="btn" type="button" onClick={dismissConnectDiff}>
                {copy("Got it", "知道了", "瞭解了", "了解しました")}
              </button>
            </div>
            <p className="connect-diff-intro">
              {copy("Only the ", "只改了让请求经本地网关必需的", "只改了讓請求經本地閘道器必需的", "リクエストがローカルゲートウェイを経由する必要のある")}
              <strong>{copy("required routing fields", "这几个关键字段", "幾個關鍵欄位", "いくつかの重要なフィールド")}</strong>
              {copy(
                " were changed. Your original configuration was backed up and can be restored when disconnecting:",
                "，你的原配置已自动备份，断开时一键还原：", "被修改。您的原配置已自動備份，斷開時可一鍵還原：", "が変更されました。元の設定は自動的にバックアップされ、切断時に1クリックで復元できます："
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
              `另有 ${rest} 项辅助开关调整。`, `另有 ${rest} 項輔助開關調整。`, `${rest} 項の補助スイッチが調整されました。`
            )}</p>}
            <p className="connect-diff-foot">{copy(
              "This notice appears only on the first connection.",
              "此提示仅在首次接入时出现一次，之后不再打扰。", "此提示僅在首次接入時出現一次，之後不再打擾。", "この通知は最初の接続時のみ表示され、その後は表示されません。"
            )}</p>
          </section>
        );
      })()}
        </>
      )}
      {pageMode !== "connection" && (
        !independentEditorOpen ? (
          <section
            className="panel agent-route-inheritance-card"
            aria-labelledby={`agent-route-inheritance-${metadata.agent_id}`}
          >
            <span className="agent-route-inheritance-mark" aria-hidden="true">
              <Globe2 />
            </span>
            <div className="agent-route-inheritance-copy">
              <span className="agent-route-inheritance-kicker">{copy("Routing source", "路由来源", "路由來源", "ルーティングソース")}</span>
              <h2 id={`agent-route-inheritance-${metadata.agent_id}`}>{copy("Follows global routing", "跟随全局路由", "跟隨全域路由", "グローバルルーティングに従う")}</h2>
              <p>{copy(
                `${metadata.display_name} automatically uses the global configuration. Global changes apply here without duplicate maintenance.`,
                `${metadata.display_name} 自动使用全局配置；全局路由变化后，无需在这里重复维护。`,
                `${metadata.display_name} 自動使用全域設定；全域路由變更後，無需在此重複維護。`,
                `${metadata.display_name} はグローバル設定を自動的に使用します。グローバル変更は重複した設定なしで反映されます。`,
              )}</p>
              <div className="agent-route-inheritance-current">
                <span>{copy("Current strategy", "当前策略", "目前策略", "現在の戦略")}</span>
                <strong>{inheritedStrategyName}</strong>
                <code>{inheritedStrategyDetail}</code>
              </div>
            </div>
            <Button
              className="agent-route-inheritance-action"
              variant="outline"
              type="button"
              onClick={() => setIndependentEditorOpen(true)}
            >
              <SlidersHorizontal data-icon="inline-start" aria-hidden="true" />
              {copy("Set independent routing", "设置独立路由", "設定獨立路由", "独立ルーティングを設定")}
            </Button>
          </section>
        ) : (
        <>
      <div className="agent-route-editor-bar">
        <div>
          <strong>{copy("Independent routing", "独立路由设置", "獨立路由設定", "独立ルーティング設定")}</strong>
          <small>{copy(
            `Set an independent route for ${metadata.display_name}; shared provider and quota data remain global.`,
            `为 ${metadata.display_name} 设置独立路由；供应商与额度数据仍由全局统一管理。`,
            `為 ${metadata.display_name} 設定獨立路由；供應商與額度資料仍由全域統一管理。`,
            `${metadata.display_name} に独立ルートを設定します。共有プロバイダーとクォータデータはグローバル管理のままです。`,
          )}</small>
        </div>
        <Button
          variant="ghost"
          type="button"
          disabled={busy}
          onClick={() => followsGlobal
            ? setIndependentEditorOpen(false)
            : void restoreHome()}
        >
          {followsGlobal
            ? copy("Collapse settings", "收起设置", "收起設定", "設定を閉じる")
            : copy("Restore global routing", "恢复跟随全局", "恢復跟隨全域", "グローバルルーティングに戻す")}
        </Button>
      </div>
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
            <h2>{copy("Three-tier routing", "三档路由", "三檔路由", "三段階ルーティング")}</h2>
            <p className="sub">{copy(
              "Provider and model selection matches Home. This only determines whether the Agent uses a custom configuration.",
              "供应商和模型选择与主页一致，只决定当前客户端是否使用独立配置。", "供應商和模型選擇與首頁一致，只決定當前客戶端是否使用獨立配置。", "プロバイダーとモデルの選択はホームと一致し、現在のクライアントが独自の設定を使用するかどうかを決定します。"
            )}</p>
          </div>
          <div className="agent-tier-heading-actions">
            <div className="mode-switch" role="radiogroup" aria-label={copy("Agent routing mode", "Agent 路由模式", "Agent 路由模式", "Agent ルーティングモード")}>
              <button type="button" role="radio" aria-checked={route.mode === "inherit"} className={route.mode === "inherit" ? "active" : ""} disabled={busy} onClick={() => void switchMode("inherit")}>{copy("Follow Home", "跟随主页", "跟隨首頁", "ホームに従う")}</button>
              <button type="button" role="radio" aria-checked={route.mode === "custom"} className={route.mode === "custom" ? "active" : ""} disabled={busy} onClick={() => void switchMode("custom")}>{copy("Custom tiers", "自定义三档", "自定義三檔", "カスタム三段階")}</button>
              <button type="button" role="radio" aria-checked={route.mode === "profile"} className={route.mode === "profile" ? "active" : ""} disabled={busy} onClick={() => void mountProfile()}>{copy("Use profile", "挂载策略组", "掛載策略組", "プロファイルをマウントする")}</button>
            </div>
            {route.mode === "custom" ? (
              <div className="agent-tier-apply-actions">
                <button className="btn" type="button" disabled={busy} onClick={() => void restoreHome()}>{copy("Restore home routing", "恢复主页路由", "恢復首頁路由", "ホームルーティングに戻す")}</button>
                <button className="btn primary" type="button" disabled={busy || Boolean(route.config_error)} onClick={() => void saveRoute()}>{copy("Save & restart", "保存并重启", "儲存並重啟", "保存して再起動")}</button>
              </div>
            ) : route.mode === "inherit" ? (
              <button
                className="btn primary"
                type="button"
                disabled={busy}
                onClick={() => void applyHomeRoute()}
                title={copy(
                  "Apply the current Home three-tier routing to this Agent now.",
                  "立即将主页当前三档路由应用到此 Agent。", "立即將首頁目前三階路由套用至此 Agent。", "現在、ホームの現在の三段階ルーティングをこの Agent に適用します。"
                )}
              >
                {busy ? copy("Working…", "处理中…", "處理中…", "処理中…") : copy("Apply", "应用", "應用", "適用")}
              </button>
            ) : null}
          </div>
        </div>

        {route.mode === "profile" && (
          <div className="profile-mount-select">
            <span>{copy("Current profile", "当前策略组", "當前策略組", "現在のプロファイル")}</span>
            <div className="profile-mount-controls">
            <Select
              value={route.profile ?? ""}
              disabled={busy}
              onValueChange={(profile) => void mountProfile(profile)}
            >
              <SelectTrigger aria-label={copy("Current profile", "当前策略组", "當前策略組", "現在のプロファイル")}>
                <SelectValue placeholder={copy("Choose a profile", "选择策略组", "選擇策略組", "プロファイルを選択")} />
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
                `删除策略组 ${route.profile ?? ""}`, `刪除策略組 ${route.profile ?? ""}`, `プロファイル ${route.profile ?? ""} を削除`
              )}
              onClick={() => void removeCurrentProfile()}
            >
              {copy("Delete", "删除", "刪除", "削除")}
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
              `该 Agent 使用策略组「${route.profile}」。在主页管理策略组，保存并应用后生效。`, `此 Agent 使用策略組「${route.profile}」。在首頁管理策略組，儲存並應用後生效。`, `この Agent はプロファイル「${route.profile}」を使用しています。ホームでプロファイルを管理し、保存して適用することで有効になります。`
            )}</span>
          ) : (
            route.mode === "inherit" ? (
              <span className="inherit-note">{copy(
                "This Agent automatically uses the latest three-tier configuration from Home.",
                "主页路由更新后，此 Agent 会自动使用最新三档配置。", "此 Agent 會自動使用來自首頁的最新三階配置。", "この Agent はホームからの最新の三段階設定を自動的に使用します。"
              )}</span>
            ) : null
          )}
          {route.config_error && <span className="foot-hint error-text">{humanizeAppError(route.config_error)}</span>}
        </footer>
      </section>
      )}
        </>
        )
      )}
    </div>
  );
}
