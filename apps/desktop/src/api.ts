import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export interface TierView {
  upstream: string | null;
  model: string | null;
}

/** A model's declared four-state capabilities (what the router gates on). */
export interface ModelCapabilityView {
  model: string;
  tool: boolean;
  vision: boolean;
  json_schema: boolean;
  /** 0 表示适配器未申报(未知),路由据此不往这里发长上下文。 */
  context_window: number;
}

export interface ProviderView {
  name: string;
  provider: string;
  base_url: string;
  models: string[];
  /** 每个模型的四态能力;后端总会给,旧 mock 可能缺,UI 兜 `?? []`。 */
  model_details?: ModelCapabilityView[];
  has_auth: boolean;
}

export interface ModelDiscoveryView {
  models: string[];
  source: "live" | "cache" | "none";
  fetched_at_ms: number | null;
  warning: string | null;
}

export type ServePhase = "stopped" | "starting" | "stopping" | "running" | "error";

export interface ServeView {
  phase: ServePhase;
  running: boolean;
  listen: string;
  virtual_key: string | null;
  error: string | null;
}

export type TierSlot = "high" | "mid" | "low";

export type AgentRouteMode = "inherit" | "custom" | "profile";

export interface AgentRouteView {
  mode: AgentRouteMode;
  tiers: Record<TierSlot, TierView>;
  config_error: string | null;
  /** 挂载的策略组名(mode === "profile" 时)。 */
  profile: string | null;
}

export interface SettingsView {
  listen: string;
  auth: boolean;
  metrics: boolean;
  data_dir: string;
  plugins_dir: string;
  agent: string;
  version: string;
}

export interface StateView {
  providers: ProviderView[];
  tiers: Record<TierSlot, TierView>;
  agent_routes: Record<string, AgentRouteView>;
  serve: ServeView;
  config_error: string | null;
  settings: SettingsView;
  /** 草稿与已保存到磁盘的配置不同 = 有未保存更改。 */
  dirty: boolean;
  /** 运行中的代理用的正是已保存那份配置(未运行时 true)。false = 已保存尚未应用。 */
  applied: boolean;
  /** 已配置的命名策略组(策略组名),供 Agent 挂载。 */
  profiles: string[];
}

export type AgentId = string;
export type AgentAdmission = "supported" | "discovery_only";
export type AgentPlatform = "macos" | "linux" | "windows" | "wsl";
export type AgentStatus =
  | "NOT_DETECTED"
  | "DETECTED_VERIFIED"
  | "DETECTED_INFERRED"
  | "DETECTED_UNKNOWN"
  | "DETECTED_BLOCKED"
  | "INSTALLED_BROKEN"
  | "MULTIPLE_INSTALLATIONS"
  | "CONNECTED";
export type AgentPlanIntent = "connect" | "disconnect" | "restore";
export type AgentConfirmationKind =
  | "installation"
  | "target_config"
  | "configuration_diff"
  | "experimental_compatibility";

export interface AgentUiMetadataView {
  agent_id: AgentId;
  legacy_kind: string | null;
  display_name: string;
  icon_key: string;
  admission: AgentAdmission;
}

export interface AgentDiagnosticView {
  reason_code: string;
  message: string;
}

export interface AgentDiscoveryView {
  agent_id: AgentId;
  executable_path: string;
  canonical_path: string;
  version_raw: string | null;
  version_normalized: string | null;
  environment: AgentPlatform;
  evidence: Array<{
    source: "known_path" | "path" | "env_override";
    observed_path: string;
    is_path_default: boolean;
  }>;
  is_path_default: boolean;
  runnable: boolean;
  config_candidates: string[];
  config_fingerprint: string | null;
  conflict_group: string | null;
  diagnostics: AgentDiagnosticView[];
  scanned_at_ms: number;
}

export interface AgentCompatibilityView {
  agent_id: AgentId;
  installation_path: string | null;
  status: AgentStatus;
  reason_code: string;
  message: string;
  matched_catalog_version: string | null;
  connector_id: string | null;
  allowed_actions: string[];
}

export interface AgentInstallationView {
  discovery: AgentDiscoveryView;
  compatibility: AgentCompatibilityView;
  connected: boolean;
}

export interface AgentView {
  metadata: AgentUiMetadataView;
  installations: AgentInstallationView[];
  status: AgentStatus;
  catalog_sequence: number;
  catalog_expires_at_ms: number | null;
  catalog_source: "builtin" | "remote";
  catalog_warning: string | null;
}

export interface ConfigPlanView {
  schema_version: number;
  operation_id: string;
  intent: AgentPlanIntent;
  agent_id: AgentId;
  installation_path: string;
  target_config_path: string;
  target_existed: boolean;
  before_hash: string;
  expected_after_hash: string;
  owned_paths: Array<{ segments: string[] }>;
  changes: Array<{
    operation: "add" | "replace" | "remove" | "test";
    path: { segments: string[] };
    sensitive: boolean;
    summary: string;
  }>;
  human_diff: string;
  connector_id: string;
  compatibility_evidence: AgentCompatibilityView;
  compatibility_sequence: number;
  compatibility_expires_at_ms: number | null;
  created_at_ms: number;
  expires_at_ms: number;
  required_confirmations: AgentConfirmationKind[];
  confirmation_token: string;
}

export interface AgentOperationView {
  operation_id: string;
  agent_id: AgentId;
  target_config_path: string;
  before_hash: string;
  after_hash: string;
  snapshot_id: string;
  ownership_revision: number;
  maintenance_warning: string | null;
}

export interface SnapshotView {
  snapshot_id: string;
  agent_id: AgentId;
  target_config_path: string;
  created_at_ms: number;
  connector_id: string;
  app_version: string;
  original_existed: boolean;
  pinned: boolean;
  source: "encrypted" | "legacy_backup";
  restorable: boolean;
}

export interface AggView {
  requests: number;
  errors: number;
  p50_latency_ms: number;
  p95_latency_ms: number;
  input_tokens: number;
  output_tokens: number;
  cost_micros: number | null;
}

export interface StatsView {
  total: AggView;
  groups: [string, AggView][];
  by: string | null;
  empty: boolean;
}

export interface BandView {
  at_least: number;
  pool: string;
  upstream: string | null;
  model: string | null;
}

export interface PoolView {
  pool: string;
  upstream: string | null;
  model: string | null;
}

export interface RouterTableView {
  default_pool: string;
  assumed_context_window: number;
  threshold: number | null;
  // rules / hint_routes 原样透传,结构随内核演进,故用宽松类型。
  rules: Record<string, unknown>[];
  hint_routes: Record<string, unknown>[];
  bands: BandView[];
  pools: PoolView[];
}

export interface PluginsView {
  dir: string;
  agent: string;
  dialects: string[];
  listing: string;
}

export interface UpgradeView {
  current: string;
  latest_tag: string;
  html_url: string;
  newer: boolean;
}

export const getState = () => invoke<StateView>("get_state");

export interface EndpointPreview {
  base: string;
  chat: string;
  responses: string;
  messages: string;
}

/** The final request URLs a base URL resolves to (or a validation error). */
export const previewEndpoint = (baseUrl: string) =>
  invoke<EndpointPreview>("preview_endpoint", { baseUrl });

export const addProvider = (
  name: string,
  base_url: string,
  models: string[],
  api_key: string | null,
) => invoke<StateView>("add_provider", { name, baseUrl: base_url, models, apiKey: api_key });

export const removeProvider = (name: string) =>
  invoke<StateView>("remove_provider", { name });

export const discoverProviderModels = (
  name: string,
  base_url: string,
  api_key: string | null,
) =>
  invoke<ModelDiscoveryView>("discover_provider_models", {
    name,
    baseUrl: base_url,
    apiKey: api_key,
  });

export const updateProviderModels = (name: string, models: string[]) =>
  invoke<StateView>("update_provider_models", { name, models });

export const setTier = (
  slot: TierSlot,
  upstream: string | null,
  model: string | null,
) => invoke<StateView>("set_tier", { slot, upstream, model });

export const setAgentRouteMode = (agentId: AgentId, mode: AgentRouteMode) =>
  invoke<StateView>("set_agent_route_mode", { agentId, mode });

export const saveHomeRouteAsProfile = (name: string) =>
  invoke<StateView>("save_home_route_as_profile", { name });

export const mountAgentProfile = (agentId: AgentId, profile: string) =>
  invoke<StateView>("mount_agent_profile", { agentId, profile });

export const deleteProfile = (name: string) =>
  invoke<StateView>("delete_profile", { name });

export const setAgentTier = (
  agentId: AgentId,
  slot: TierSlot,
  upstream: string | null,
  model: string | null,
) => invoke<StateView>("set_agent_tier", { agentId, slot, upstream, model });

export const saveAgentRoutes = () => invoke<StateView>("save_agent_routes");

export const applyHomeRouteToAllAgents = () =>
  invoke<StateView>("apply_home_route_to_all_agents");

export const saveConfig = () => invoke<StateView>("save_config");

export const serveStart = () => invoke<StateView>("serve_start");
export const serveStop = () => invoke<StateView>("serve_stop");

export const listenServeState = (handler: (serve: ServeView) => void) =>
  listen<ServeView>("serve-state-changed", (event) => handler(event.payload));

export const listAgentRegistry = () =>
  invoke<AgentUiMetadataView[]>("list_agent_registry");

export const scanAgents = () => invoke<AgentView[]>("scan_agents");

export const planAgentConnection = (
  agentId: AgentId,
  installationPath: string,
  options?: { expectedVersion: string },
) => invoke<ConfigPlanView>("plan_agent_connection", {
  agentId,
  installationPath,
  ...(options ? options : {}),
});

export const applyAgentPlan = (
  operationId: string,
  confirmationToken: string,
  experimentalCompatibilityConfirmed = false,
) => invoke<AgentOperationView>("apply_agent_plan", {
  operationId,
  confirmationToken,
  ...(experimentalCompatibilityConfirmed ? { experimentalCompatibilityConfirmed: true } : {}),
});

export const planAgentDisconnect = (agentId: AgentId, installationPath: string) =>
  invoke<ConfigPlanView>("plan_agent_disconnect", { agentId, installationPath });

export const listAgentSnapshots = (agentId: AgentId) =>
  invoke<SnapshotView[]>("list_agent_snapshots", { agentId });

export const planSnapshotRestore = (snapshotId: string) =>
  invoke<ConfigPlanView>("plan_snapshot_restore", { snapshotId });

export const applySnapshotRestore = (operationId: string, confirmationToken: string) =>
  invoke<AgentOperationView>("apply_snapshot_restore", {
    operationId,
    confirmationToken,
  });

export const setSettings = (auth: boolean, metrics: boolean) =>
  invoke<StateView>("set_settings", { auth, metrics });

// ---------------------------------------------------------------------------
// 数据面(只读):优先走本地 HTTP `/admin/*`,让同一份前端脱离 Tauri 壳也能跑
// (浏览器直连 dev、将来远程管理台)。代理没起或请求失败时,在 Tauri 壳内回退
// IPC——这样「代理已停止」时用量/路由表页仍然可用(读草稿与本地库),行为与
// 改造前一致。特权操作(Agent 事务 / 写配置 / 密钥)只走 IPC,永不上 HTTP。

const IN_TAURI = "__TAURI_INTERNALS__" in window;

let adminBase: string | null = null;
let adminKey: string | null = null;

/** App 每次刷新状态时同步数据面端点(App.tsx 调用)。 */
export function setAdminEndpoint(serve: ServeView) {
  adminBase = serve.phase === "running" ? `http://${serve.listen}` : null;
  adminKey = serve.phase === "running" ? serve.virtual_key : null;
}

// 纯浏览器模式(无 Tauri 壳)没有 get_state 可问:从 localStorage 取端点,
// 默认本机默认端口。用法:localStorage.setItem("ts_listen","127.0.0.1:8787");
// localStorage.setItem("ts_key","<虚拟key>") 后刷新页面。
if (!IN_TAURI) {
  adminBase = `http://${localStorage.getItem("ts_listen") ?? "127.0.0.1:8787"}`;
  adminKey = localStorage.getItem("ts_key");
}

async function dataGet<T>(path: string, ipcFallback: () => Promise<T>): Promise<T> {
  if (adminBase) {
    try {
      const response = await fetch(adminBase + path, {
        headers: adminKey ? { authorization: `Bearer ${adminKey}` } : {},
      });
      if (response.ok) return (await response.json()) as T;
      // 非 2xx(如 key 失效)也回退 IPC;纯浏览器下直接报错。
    } catch {
      // 网络失败(代理刚停等):走回退。
    }
  }
  if (IN_TAURI) return ipcFallback();
  throw new Error(
    "无法连接本地代理:请确认 token-station serve 已启动,并在 localStorage 配置 ts_listen / ts_key",
  );
}

export const getStats = (since: string, by: string | null) =>
  dataGet<StatsView>(
    `/admin/stats?since=${since}${by ? `&by=${by}` : ""}`,
    () => invoke<StatsView>("get_stats", { since, by }),
  );

/** One request's routing receipt — content-free. */
export interface Receipt {
  request_id: string;
  started_at_ms: number;
  latency_ms: number;
  status: number;
  error_code: string | null;
  /** 具体不满足的能力(缺哪一维),细化 error_code;无则 null。 */
  error_detail: string | null;
  requested_model: string;
  upstream: string | null;
  model: string | null;
  pool: string | null;
  tier: string | null;
  attempts: number;
  cost_micros: number | null;
}

export const getReceipts = (limit: number) =>
  invoke<Receipt[]>("get_receipts", { limit });

// 注意语义差:HTTP 返回**运行中**配置的路由表,IPC 回退返回可编辑草稿。
// 代理运行时以运行态为准,正是数据面该报告的事实。
export const getRouterTable = () =>
  dataGet<RouterTableView>("/admin/router-table", () =>
    invoke<RouterTableView>("get_router_table"),
  );

export const getPlugins = () =>
  dataGet<PluginsView>("/admin/plugins", () => invoke<PluginsView>("get_plugins"));

export const checkUpgrade = () => invoke<UpgradeView>("check_upgrade");
