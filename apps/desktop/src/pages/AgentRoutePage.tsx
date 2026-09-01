import { useEffect, useMemo, useState } from "react";
import {
  AlertTriangle,
  Check as CheckIcon,
  Copy as CopyIcon,
  FileDiff,
  FolderOpen,
  Globe2,
  Route as RouteIcon,
  ShieldCheck,
  SlidersHorizontal,
} from "lucide-react";
import {
  applyAgentPlan,
  configureCursorProvider,
  discardAgentPlan,
  ensureServeRunning,
  getAgentDrift,
  getAgentBackupDirectory,
  getCursorProviderStatus,
  mountAgentProfile,
  openAgentBackupDirectory,
  planAgentConnection,
  planAgentDisconnect,
  restartAgentRoute,
  restoreCursorProvider,
  setAgentRouteMode,
  setAgentTier,
  type AgentInstallationView,
  type ConfigPlanView,
  type AgentDriftView,
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
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "../components/ui/dialog";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "../components/ui/alert-dialog";
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

function errorText(error: unknown) {
  return humanizeAppError(error);
}

function planFiles(plan: ConfigPlanView, direction: "forward" | "reverse" = "forward") {
  if (plan.projection?.files?.length) {
    return plan.projection.files.map((file) => ({
      path: file.target_config_path,
      changes: direction === "reverse" ? file.reverse_changes : file.forward_changes,
    }));
  }
  return [{ path: plan.target_config_path, changes: plan.changes ?? [] }];
}

function changeState(
  operation: ConfigPlanView["changes"][number]["operation"],
  side: "before" | "after",
  intent: "connect" | "restore" | "review",
  preview: string | undefined,
  copy: LocalizedCopy,
) {
  if (preview !== undefined) return preview;
  if (side === "before") {
    return copy("Not set", "未设置", "未設定", "未設定");
  }
  if (operation === "remove") return copy("Remove this field", "删除此字段", "刪除此欄位", "このフィールドを削除");
  if (operation === "test") return copy("Keep unchanged", "保持不变", "保持不變", "変更しない");
  return intent === "restore"
    ? copy("Value from the pre-connection backup", "接入前备份值", "連線前備份值", "接続前のバックアップ値")
    : copy("Token Station managed value", "Token Station 受管值", "Token Station 受管值", "Token Station 管理値");
}

function changeValueMeaning(path: string, preview: string | undefined, copy: LocalizedCopy) {
  if (preview === '"0"' && path === "env.MAX_THINKING_TOKENS") {
    return copy("No Thinking token budget", "Thinking token 预算设为 0", "Thinking token 預算設為 0", "Thinking token 予算を 0 に設定");
  }
  if (preview !== '"1"') return null;
  switch (path) {
    case "env.CLAUDE_CODE_DISABLE_THINKING":
      return copy("Disable Thinking", "关闭 Thinking", "關閉 Thinking", "Thinking を無効化");
    case "env.CLAUDE_CODE_DISABLE_ADAPTIVE_THINKING":
      return copy("Disable adaptive Thinking", "关闭自适应 Thinking", "關閉自適應 Thinking", "アダプティブ Thinking を無効化");
    case "env.CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS":
      return copy("Disable experimental beta features", "关闭实验性 Beta 功能", "關閉實驗性 Beta 功能", "実験的な Beta 機能を無効化");
    case "env.CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC":
      return copy("Disable nonessential network traffic", "关闭非必要网络请求", "關閉非必要網路請求", "必須でないネットワーク通信を無効化");
    default:
      return null;
  }
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

function connectedRouteDetail(route: AgentRouteView, copy: LocalizedCopy): string {
  if (route.routing_mode === "direct") {
    const upstream = route.direct_target?.upstream;
    const model = route.direct_target?.model;
    if (upstream && model) {
      return copy(
        `Gateway ready · ${upstream} / ${model}`,
        `网关正常 · ${upstream} / ${model}`,
        `閘道正常 · ${upstream} / ${model}`,
        `ゲートウェイ正常 · ${upstream} / ${model}`,
      );
    }
    return copy(
      "Connected to the local gateway. The Direct route is incomplete.",
      "已连接本机网关。当前直连路由尚未完整配置。",
      "已連接本機閨道。目前直連路由尚未完整設定。",
      "ローカルゲートウェイに接続済みです。ダイレクトルートの設定が完了していません。",
    );
  }
  if (route.routing_mode === "quota_first") {
    return copy(
      "Connected to the local gateway. The current quota policy selects the Provider and model.",
      "已连接本机网关。Provider 与模型由当前额度策略选择。",
      "已連接本機閨道。Provider 與模型由目前額度策略選擇。",
      "ローカルゲートウェイに接続済みです。Provider とモデルは現在のクォータポリシーで選択されます。",
    );
  }
  return copy(
    "Connected to the local gateway. The current tier rules select the Provider and model.",
    "已连接本机网关。Provider 与模型由当前分档规则选择。",
    "已連接本機閨道。Provider 與模型由目前分檔規則選擇。",
    "ローカルゲートウェイに接続済みです。Provider とモデルは現在の階層ルールで選択されます。",
  );
}

function statusCopy(
  metadata: AgentUiMetadataView,
  agent: AgentView | undefined,
  installation: AgentInstallationView | undefined,
  route: AgentRouteView,
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
        "Multiple installations found. Select a version.",
        "检测到多份安装，请选择版本。", "檢測到多份安裝，請選擇版本。", "複数のインストールがあります。バージョンを選択してください。"
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
      detail: connectedRouteDetail(route, copy),
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
  const [pendingPlan, setPendingPlan] = useState<{
    intent: "connect" | "restore" | "review";
    plan: ConfigPlanView;
  } | null>(null);
  const [restoreConflict, setRestoreConflict] = useState<AgentDriftView[] | null>(null);
  const [cursorRestorePending, setCursorRestorePending] = useState(false);
  const [copiedDiscoveryPath, setCopiedDiscoveryPath] = useState<{ path: string } | null>(null);
  const [backupDirectory, setBackupDirectory] = useState("");
  const [backupDirectoryCopied, setBackupDirectoryCopied] = useState(false);
  const followsGlobal = route.inherits_global === true;
  const [independentEditorOpen, setIndependentEditorOpen] = useState(!followsGlobal);

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
    let cancelled = false;
    void getAgentBackupDirectory()
      .then((path) => {
        if (!cancelled) setBackupDirectory(path);
      })
      .catch((caught) => {
        if (!cancelled) showError(errorText(caught), "agent-backup-directory");
      });
    return () => {
      cancelled = true;
    };
  }, [showError]);

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

  const discoveredStatus = statusCopy(metadata, agent, installation, route, copy, language);
  const status = metadata.agent_id === "cursor" && cursorStatus?.state === "connected"
    ? {
      tone: "success" as const,
      label: copy("Connected", "已接入", "已接入", "接続済み"),
      detail: connectedRouteDetail(route, copy),
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
  const routeNeedsAttention = Boolean(route.config_error || !route.direct_target?.model);
  const discoveredPath = installation?.discovery.canonical_path;
  const discoveredPathCopied = copiedDiscoveryPath?.path === discoveredPath;
  const installationVersion = installation?.discovery.version_normalized
    ?? installation?.discovery.version_raw
    ?? ((agent?.installations.length ?? 0) > 1
      ? copy("Not selected", "未选择", "未選擇", "選択されていません")
      : copy("Unknown", "未知", "未知", "不明"));
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

  const copyBackupDirectory = async () => {
    if (!backupDirectory) return;
    try {
      await navigator.clipboard.writeText(backupDirectory);
      setBackupDirectoryCopied(true);
      window.setTimeout(() => setBackupDirectoryCopied(false), 1_600);
    } catch {
      showError(
        copy("Unable to copy the backup directory.", "无法复制备份目录。", "無法複製備份目錄。", "バックアップディレクトリをコピーできません。"),
        "agent-backup-directory-copy",
      );
    }
  };

  const openBackupDirectory = async () => {
    try {
      await openAgentBackupDirectory();
    } catch (caught) {
      showError(errorText(caught), "agent-backup-directory-open");
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

  const previewConnection = async () => {
    if (!installation || !canOperate || busy) return;
    let planReady = false;
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
      setPendingPlan({ intent: "connect", plan });
      planReady = true;
    } catch (caught) {
      showError(errorText(caught), `agent-connect:${metadata.agent_id}`);
    } finally {
      onConnectInFlightChange?.(false);
      setBusy(false);
      if (!planReady) {
        try {
          await onRefreshAgents();
        } catch (caught) {
          showError(errorText(caught), `agent-refresh:${metadata.agent_id}`);
        }
      }
    }
  };

  const applyPendingPlan = async () => {
    if (!pendingPlan || pendingPlan.intent === "review" || busy) return;
    const { intent, plan } = pendingPlan;
    setBusy(true);
    try {
      await applyAgentPlan(plan.operation_id, plan.confirmation_token);
      setPendingPlan(null);
      showSuccess(
        intent === "connect"
          ? copy("Agent connected.", "Agent 已接入。", "Agent 已連線。", "Agent が接続されました。")
          : copy("Restored the backup and disconnected.", "已恢复备份并断开。", "已恢復備份並斷開。", "バックアップを復元して接続を解除しました。"),
        `agent-${intent}:${metadata.agent_id}`,
      );
      try {
        await onRefreshAgents();
      } catch (caught) {
        showError(errorText(caught), `agent-refresh:${metadata.agent_id}`);
      }
    } catch (caught) {
      setPendingPlan(null);
      showError(errorText(caught), `agent-${intent}:${metadata.agent_id}`);
      try {
        await onRefreshAgents();
      } catch (refreshError) {
        showError(errorText(refreshError), `agent-refresh:${metadata.agent_id}`);
      }
    } finally {
      setBusy(false);
    }
  };

  const closePendingPlan = () => {
    const closing = pendingPlan;
    setPendingPlan(null);
    if (closing) {
      void discardAgentPlan(
        closing.plan.operation_id,
        closing.plan.confirmation_token,
      ).catch(() => undefined);
    }
  };

  const reviewConnectionChanges = async () => {
    if (!installation || !managed || metadata.agent_id === "cursor" || busy) return;
    setBusy(true);
    try {
      const plan = await planAgentDisconnect(
        metadata.agent_id,
        installation.discovery.canonical_path,
      );
      setPendingPlan({ intent: "review", plan });
    } catch (caught) {
      showError(errorText(caught), `agent-review-connection:${metadata.agent_id}`);
    } finally {
      setBusy(false);
    }
  };

  const applyCursorRestore = async () => {
    if (busy) return;
    setCursorRestorePending(false);
    setBusy(true);
    try {
      const next = await restoreCursorProvider();
      setCursorStatus(next);
      showSuccess(
        next.message ?? copy(
          "Restored the official Cursor configuration and disconnected.",
          "已恢复 Cursor 官方配置并断开。", "已恢復 Cursor 官方設定並斷開。", "Cursor の公式設定を復元し、接続を解除しました。"
        ),
        `agent-restore-official:${metadata.agent_id}`,
      );
      try {
        await onRefreshAgents();
      } catch (caught) {
        showError(errorText(caught), `agent-refresh:${metadata.agent_id}`);
      }
    } catch (caught) {
      showError(errorText(caught), `agent-restore-official:${metadata.agent_id}`);
    } finally {
      setBusy(false);
    }
  };

  const createRestorePlan = async (applyImmediately: boolean) => {
    if (!installation || busy) return;
    setBusy(true);
    try {
      const plan = await planAgentDisconnect(
        metadata.agent_id,
        installation.discovery.canonical_path,
      );
      if (!applyImmediately) {
        setPendingPlan({ intent: "restore", plan });
        return;
      }
      await applyAgentPlan(plan.operation_id, plan.confirmation_token);
      setRestoreConflict(null);
      showSuccess(
        copy(
          "Restored the backup and disconnected.",
          "已强制恢复备份并断开。", "已強制恢復備份並斷開。", "バックアップを強制復元して接続を解除しました。"
        ),
        `agent-restore-official:${metadata.agent_id}`,
      );
      try {
        await onRefreshAgents();
      } catch (caught) {
        showError(errorText(caught), `agent-refresh:${metadata.agent_id}`);
      }
    } catch (caught) {
      showError(errorText(caught), `agent-restore-official:${metadata.agent_id}`);
    } finally {
      setBusy(false);
    }
  };

  const restoreOfficial = async () => {
    if (!installation || busy) return;
    if (metadata.agent_id === "cursor") {
      setCursorRestorePending(true);
      return;
    }
    setBusy(true);
    try {
      const drift = await getAgentDrift(
        metadata.agent_id,
        installation.discovery.canonical_path,
      );
      const changed = drift.filter((item) => item.status !== "in_sync");
      if (changed.length > 0) {
        setRestoreConflict(changed);
      } else {
        const plan = await planAgentDisconnect(
          metadata.agent_id,
          installation.discovery.canonical_path,
        );
        setPendingPlan({ intent: "restore", plan });
      }
    } catch (caught) {
      showError(errorText(caught), `agent-restore-official:${metadata.agent_id}`);
    } finally {
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
      const next = await restartAgentRoute(metadata.agent_id);
      onStateChange(next);
      setIndependentEditorOpen(false);
      showSuccess(
        serveRunning
          ? copy("Restored and applied home routing", "已恢复并应用主页路由", "已恢復並套用首頁路由", "ホームルーティングを復元して適用しました")
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
      <header className="agent-route-hero agent-flat-surface">
        <div className="agent-identity">
          <span className="agent-large-mark" aria-hidden="true">
            <AgentIcon
              id={metadata.agent_id}
              fallback={metadata.nav_mark ?? metadata.display_name.slice(0, 1)}
              size={50}
            />
          </span>
          <div className="agent-identity-copy">
            {embedded ? <h2>{metadata.display_name}</h2> : <h1>{metadata.display_name}</h1>}
            <span className="agent-identity-version">{installationVersion}</span>
          </div>
        </div>
        <div className="agent-connect-box">
          <div className="agent-connect-status">
            <span className={`status-chip ${status.tone}`}>{status.label}</span>
            <small title={status.detail}>{status.detail}</small>
          </div>
          <div className="agent-connect-actions">
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
          <Button
            className="agent-primary-action"
            variant={managed ? "secondary" : "default"}
            size="lg"
            type="button"
            data-onboarding-target={!managed ? "agent-connect" : undefined}
            disabled={busy || !canOperate}
            onClick={() => void (managed ? restoreOfficial() : previewConnection())}
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
                    : copy("Preview & connect", "预览并接入", "預覽並連線", "プレビューして接続")}
          </Button>
          {pageMode !== "connection" && cursorRepairRequired ? (
            <Button
              className="agent-secondary-action"
              variant="secondary"
              type="button"
              disabled={busy || !installation}
              onClick={() => void restoreOfficial()}
              title={copy(
                "Restore the original Cursor configuration and clear the stale management record.",
                "恢复 Cursor 原配置，并清除失效的接管记录。", "恢復 Cursor 原設定，並清除失效的接管記錄。", "Cursor の元の設定に戻し、無効な管理記録を削除します。"
              )}
            >
              {copy("Restore official configuration & disconnect", "恢复官方配置并断开", "恢復官方配置並斷開", "公式設定を復元し、接続を解除")}
            </Button>
          ) : null}
          </div>
        </div>
      </header>

      <section className="agent-connection-detail agent-flat-surface" aria-label={copy("Agent connection details", "Agent 接入详情", "Agent 連線詳情", "Agent 接続詳細")}>
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
        </dl>
        <div className="agent-connection-change">
          <div>
            <h3>{copy("Safe configuration change", "安全修改配置", "安全修改設定", "安全な設定変更")}</h3>
            {managed && metadata.agent_id !== "cursor" ? (
              <Button
                className="agent-review-changes"
                variant="outline"
                size="sm"
                type="button"
                disabled={busy || !installation}
                onClick={() => void reviewConnectionChanges()}
              >
                <FileDiff aria-hidden="true" />
                {copy("Review connection changes", "查看接入改动", "查看接入變更", "接続変更を確認")}
              </Button>
            ) : (
              <p>{copy("Preview first. Back up second. Write only after confirmation.", "先预览，再备份，确认后才写入。", "先預覽，再備份，確認後才寫入。", "先にプレビューし、次にバックアップし、確認後のみ書き込みます。")}</p>
            )}
          </div>
          <div className="agent-connection-file">
            <code>{connectionTarget}</code>
          </div>
          <div className="agent-backup-location">
            <div>
              <span>{copy("Encrypted backup directory", "加密备份目录", "加密備份目錄", "暗号化バックアップディレクトリ")}</span>
              <code>{backupDirectory || copy("Loading…", "正在获取…", "正在取得…", "読み込み中…")}</code>
              <small className="agent-backup-layout-note">{copy(
                "Each Agent has its own folder. File names include the time and Agent ID.",
                "每个 Agent 一个文件夹，文件名包含时间和 Agent 名称。",
                "每個 Agent 一個資料夾，檔名包含時間和 Agent 名稱。",
                "Agent ごとにフォルダを分け、ファイル名に時刻と Agent 名を含めます。",
              )}</small>
            </div>
            <div className="agent-backup-location-actions">
              <TooltipProvider>
                <Tooltip>
                  <TooltipTrigger asChild>
                    <Button variant="ghost" size="icon-sm" type="button" disabled={!backupDirectory} aria-label={backupDirectoryCopied
                      ? copy("Backup directory copied", "备份目录已复制", "備份目錄已複製", "バックアップディレクトリをコピーしました")
                      : copy("Copy backup directory", "复制备份目录", "複製備份目錄", "バックアップディレクトリをコピー")}
                      onClick={() => void copyBackupDirectory()}>
                      {backupDirectoryCopied
                        ? <CheckIcon data-icon="inline-start" aria-hidden="true" />
                        : <CopyIcon data-icon="inline-start" aria-hidden="true" />}
                    </Button>
                  </TooltipTrigger>
                  <TooltipContent>{copy("Copy backup directory", "复制备份目录", "複製備份目錄", "バックアップディレクトリをコピー")}</TooltipContent>
                </Tooltip>
                <Tooltip>
                  <TooltipTrigger asChild>
                    <Button variant="ghost" size="icon-sm" type="button" disabled={!backupDirectory}
                      aria-label={copy("Open backup folder", "打开备份文件夹", "開啟備份資料夾", "バックアップフォルダを開く")}
                      onClick={() => void openBackupDirectory()}>
                      <FolderOpen data-icon="inline-start" aria-hidden="true" />
                    </Button>
                  </TooltipTrigger>
                  <TooltipContent>{copy("Open backup folder", "打开备份文件夹", "開啟備份資料夾", "バックアップフォルダを開く")}</TooltipContent>
                </Tooltip>
              </TooltipProvider>
            </div>
          </div>
          <details className="agent-backup-policy">
            <summary>{copy("How backup and restore work", "备份与恢复如何工作", "備份與恢復如何運作", "バックアップと復元の仕組み")}</summary>
            <ol>
              <li>{copy("Show the target file and field-level changes.", "展示目标文件与字段级改动。", "展示目標檔案與欄位級變更。", "対象ファイルとフィールド単位の変更を表示します。")}</li>
              <li>{copy("Create an encrypted local snapshot immediately before writing.", "写入前立即创建本机加密快照。", "寫入前立即建立本機加密快照。", "書き込み直前にローカル暗号化スナップショットを作成します。")}</li>
              <li>{copy("Check for later manual edits before restoring.", "恢复前检查接入后的手动修改。", "恢復前檢查連線後的手動修改。", "復元前に接続後の手動変更を確認します。")}</li>
            </ol>
          </details>
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

      <Dialog
        open={pendingPlan !== null}
        onOpenChange={(open) => {
          if (!open && !busy) closePendingPlan();
        }}
      >
        <DialogContent className="agent-change-dialog" closeLabel={copy("Close", "关闭", "關閉", "閉じる")}>
          <DialogHeader>
            <span className="agent-change-dialog-mark" aria-hidden="true"><FileDiff /></span>
            <DialogTitle>{pendingPlan?.intent === "review"
              ? copy("Connection changes", "接入改动", "接入變更", "接続変更")
              : pendingPlan?.intent === "restore"
                ? copy("Confirm backup restore", "确认恢复备份", "確認恢復備份", "バックアップの復元を確認")
                : copy("Confirm connection changes", "确认接入改动", "確認連線變更", "接続変更を確認")}</DialogTitle>
            <DialogDescription>{pendingPlan?.intent === "restore"
              ? copy(
                "Review the owned fields that will return to their pre-connection state. Local credentials appear as plain text. Avoid screenshots and screen sharing.",
                "请确认将恢复到接入前状态的受管字段。本机凭据会以明文显示，请避免截屏或共享屏幕。",
                "請確認將恢復到連線前狀態的受管欄位。本機憑證會以明文顯示，請避免截圖或共享螢幕。",
                "接続前の状態に戻す管理フィールドを確認してください。ローカル認証情報は平文で表示されます。スクリーンショットと画面共有を避けてください。"
              )
              : pendingPlan?.intent === "review"
                ? copy(
                  "These are the exact Connector-owned values Token Station changed during connection. Local credentials appear as plain text. Avoid screenshots and screen sharing.",
                  "这是 Token Station 接入时修改的确切受管值。本机凭据会以明文显示，请避免截屏或共享屏幕。",
                  "這是 Token Station 接入時修改的確切受管值。本機憑證會以明文顯示，請避免截圖或共享螢幕。",
                  "Token Station が接続時に変更した正確な管理値です。ローカル認証情報は平文で表示されます。スクリーンショットと画面共有を避けてください。"
                )
              : copy(
                "No file has been changed yet. Review every field before continuing. Local credentials appear as plain text. Avoid screenshots and screen sharing.",
                "配置文件尚未修改。请先核对每一项改动。本机凭据会以明文显示，请避免截屏或共享屏幕。",
                "設定檔案尚未修改。請先核對每一項變更。本機憑證會以明文顯示，請避免截圖或共享螢幕。",
                "設定ファイルはまだ変更されていません。続行する前に各フィールドを確認してください。ローカル認証情報は平文で表示されます。スクリーンショットと画面共有を避けてください。"
              )}</DialogDescription>
          </DialogHeader>
          <div className="agent-change-scroll" role="region" aria-label={copy("Configuration changes", "配置改动", "設定變更", "設定変更")} tabIndex={0}>
            {pendingPlan ? (
              <div className="agent-change-files">
                {planFiles(pendingPlan.plan, pendingPlan.intent === "review" ? "reverse" : "forward").map((file, fileIndex) => (
                  <section className="agent-change-file" key={`${file.path}-${fileIndex}`}>
                    <div className="agent-change-file-head">
                      <span>{copy("Target file", "目标文件", "目標檔案", "対象ファイル")}</span>
                      <code>{file.path}</code>
                    </div>
                    <div className="agent-change-list">
                      {file.changes.map((change, index) => {
                        const changePath = change.path.segments.join(".");
                        const beforePreview = change.before_preview;
                        const afterPreview = change.after_preview;
                        const beforeMeaning = changeValueMeaning(changePath, beforePreview, copy);
                        const afterMeaning = changeValueMeaning(changePath, afterPreview, copy);
                        return (
                        <article className="agent-change-row" key={`${changePath}-${index}`}>
                          <div className="agent-change-path-group">
                            <code className="agent-change-path">{changePath}</code>
                          </div>
                          <div className="agent-change-states">
                            <div>
                              <span>{copy("Before", "修改前", "修改前", "変更前")}</span>
                              <strong className={beforePreview !== undefined ? "agent-change-value" : undefined}>{changeState(change.operation, "before", pendingPlan.intent, beforePreview, copy)}</strong>
                              {beforeMeaning ? <small>{beforeMeaning}</small> : null}
                            </div>
                            <span className="agent-change-arrow" aria-hidden="true">→</span>
                            <div className="after">
                              <span>{copy("After", "修改后", "修改後", "変更後")}</span>
                              <strong className={afterPreview !== undefined ? "agent-change-value" : undefined}>{changeState(change.operation, "after", pendingPlan.intent, afterPreview, copy)}</strong>
                              {afterMeaning ? <small>{afterMeaning}</small> : null}
                            </div>
                          </div>
                        </article>
                        );
                      })}
                    </div>
                  </section>
                ))}
              </div>
            ) : null}
            <div className="agent-backup-assurance">
              <ShieldCheck aria-hidden="true" />
              <div>
                <strong>{copy("Encrypted backup", "已加密备份", "已加密備份", "暗号化バックアップ")}</strong>
                <span>{pendingPlan?.intent === "restore"
                  ? copy("Only Token Station-owned fields change. Other fields stay as they are.", "只恢复 Token Station 受管字段，其他字段保持不变。", "只恢復 Token Station 受管欄位，其他欄位保持不變。", "Token Station 管理フィールドのみ復元し、他のフィールドは保持します。")
                  : pendingPlan?.intent === "review"
                    ? copy("This is a read-only record. Closing it does not change the file.", "这是只读记录，关闭不会修改文件。", "這是唯讀記錄，關閉不會修改檔案。", "読み取り専用の記録です。閉じてもファイルは変更されません。")
                  : copy("Token Station saves the original file locally immediately before writing.", "写入前会在本机保存原文件的加密快照。", "寫入前會在本機儲存原檔案的加密快照。", "書き込み直前に元のファイルをローカルへ暗号化保存します。")}</span>
              </div>
            </div>
          </div>
          <DialogFooter>
            <Button variant="outline" type="button" disabled={busy} onClick={closePendingPlan}>
              {pendingPlan?.intent === "review"
                ? copy("Close", "关闭", "關閉", "閉じる")
                : copy("Cancel", "取消", "取消", "キャンセル")}
            </Button>
            {pendingPlan?.intent !== "review" ? <Button type="button" disabled={busy} onClick={() => void applyPendingPlan()}>
              {busy
                ? copy("Working…", "处理中…", "處理中…", "処理中…")
                : pendingPlan?.intent === "restore"
                  ? copy("Restore backup & disconnect", "恢复备份并断开", "恢復備份並斷開", "バックアップを復元して切断")
                  : copy("Confirm connection", "确认接入", "確認連線", "接続を確認")}
            </Button> : null}
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <AlertDialog open={restoreConflict !== null} onOpenChange={(open) => !open && setRestoreConflict(null)}>
        <AlertDialogContent className="agent-restore-conflict-dialog">
          <AlertDialogHeader>
            <span className="agent-restore-warning-mark" aria-hidden="true"><AlertTriangle /></span>
            <AlertDialogTitle>{copy("Configuration file was modified", "配置文件已被修改", "設定檔案已被修改", "設定ファイルが変更されています")}</AlertDialogTitle>
            <AlertDialogDescription>{copy(
              "Token Station detected changes made after connection. Choose whether to restore the backup or cancel and edit the file yourself.",
              "检测到接入后的手动修改。请选择强制恢复备份，或取消后自行修改。", "偵測到連線後的手動修改。請選擇強制恢復備份，或取消後自行修改。", "接続後の手動変更を検出しました。バックアップを強制復元するか、キャンセルして自分で編集してください。"
            )}</AlertDialogDescription>
          </AlertDialogHeader>
          <div className="agent-drift-files">
            {restoreConflict?.map((file) => {
              const managedChanges = file.changes.filter((change) => change.scope === "managed");
              const unownedChanges = file.changes.filter((change) => change.scope === "unowned");
              return (
                <section key={file.target_config_path}>
                  <code>{file.target_config_path}</code>
                  {managedChanges.length > 0 ? (
                    <div className="agent-drift-group danger">
                      <strong>{copy("Token Station managed fields", "Token Station 管理的字段", "Token Station 管理的欄位", "Token Station 管理フィールド")}</strong>
                      <span>{managedChanges.map((change) => change.path.segments.join(".")).join("、")}</span>
                    </div>
                  ) : null}
                  {unownedChanges.length > 0 ? (
                    <div className="agent-drift-group">
                      <strong>{copy("Other fields (kept)", "其他字段（将保留）", "其他欄位（將保留）", "その他のフィールド（保持）")}</strong>
                      <span>{unownedChanges.map((change) => change.path.segments.join(".")).join("、")}</span>
                    </div>
                  ) : null}
                  {file.changes.length === 0 ? <span className="agent-drift-message">{file.message}</span> : null}
                </section>
              );
            })}
          </div>
          <p className="agent-force-restore-note">{copy(
            "Force restore replaces Token Station-owned fields with their pre-connection values. Unrelated fields are preserved.",
            "强制恢复会用接入前备份覆盖受管字段；不相关字段会保留。", "強制恢復會用連線前備份覆蓋受管欄位；不相關欄位會保留。", "強制復元は管理フィールドを接続前の値に戻し、無関係なフィールドを保持します。"
          )}</p>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={busy}>{copy("I will edit it myself", "暂不恢复，我自行处理", "暫不恢復，我自行處理", "自分で編集する")}</AlertDialogCancel>
            <AlertDialogAction
              variant="destructive"
              disabled={busy}
              onClick={(event) => {
                event.preventDefault();
                void createRestorePlan(true);
              }}
            >
              {busy ? copy("Working…", "处理中…", "處理中…", "処理中…") : copy("Force restore backup", "强制恢复备份", "強制恢復備份", "バックアップを強制復元")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      <AlertDialog open={cursorRestorePending} onOpenChange={setCursorRestorePending}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{copy("Restore Cursor configuration?", "恢复 Cursor 配置？", "恢復 Cursor 設定？", "Cursor 設定を復元しますか？")}</AlertDialogTitle>
            <AlertDialogDescription>{copy(
              "Cursor uses a separate integration. Token Station will restore its saved configuration and disconnect.",
              "Cursor 使用独立接入方式。Token Station 将恢复已保存的配置并断开。", "Cursor 使用獨立連線方式。Token Station 將恢復已儲存的設定並斷開。", "Cursor は別の統合方式を使用します。保存済み設定を復元して切断します。"
            )}</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{copy("Cancel", "取消", "取消", "キャンセル")}</AlertDialogCancel>
            <AlertDialogAction onClick={() => void applyCursorRestore()}>{copy("Restore & disconnect", "恢复并断开", "恢復並斷開", "復元して切断")}</AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
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
