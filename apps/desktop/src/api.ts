import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export interface TierView {
  upstream: string | null;
  model: string | null;
}

export interface ProviderView {
  name: string;
  provider: string;
  base_url: string;
  models: string[];
  model_capabilities?: ModelCapabilityView[];
  catalog_revision?: number;
  catalog?: CatalogModelView[];
  has_auth: boolean;
}

export type CapabilityState = "verified" | "declared" | "unsupported" | "unknown";

export interface ModelCapabilityView {
  model: string;
  tool: CapabilityState;
  vision: CapabilityState;
  json_schema: CapabilityState;
}

export type CatalogSource = "live" | "cache" | "configured";
export type CatalogState = "active" | "stale" | "removed";

export interface CatalogModelView extends ModelCapabilityView {
  source: CatalogSource;
  last_seen_ms: number | null;
  catalog_state: CatalogState;
}

export interface ProviderEndpointPreview {
  chat: string;
  responses: string;
  messages: string;
}

export interface ProviderRemovalPreview {
  name: string;
  references: string[];
  can_remove: boolean;
}

export interface ProviderTestStage {
  layer: "network" | "http" | "auth" | "model" | "generation" | "stream" | "tool" | "json";
  status: "pass" | "fail" | "skipped";
  detail?: string;
}

export interface ProviderTestResult {
  model: string;
  stages: ProviderTestStage[];
  latency_ms?: number;
}

export interface ModelDiscoveryView {
  models: string[];
  source: "live" | "cache" | "none";
  fetched_at_ms: number | null;
  warning: string | null;
  revision?: number;
  catalog?: CatalogModelView[];
  added?: string[];
  removed?: string[];
}

export type ReceiptCostKind = "actual" | "estimated" | "unknown";

export type ReceiptErrorCode =
  | "invalid_request"
  | "auth"
  | "payment_required"
  | "rate_limit"
  | "capacity"
  | "capability"
  | "content_policy"
  | "upstream_unavailable"
  | "transport_truncated"
  | "context_length"
  | "provider_protocol_error"
  | "timeout"
  | "internal";

export interface ReceiptUsageView {
  input_tokens: number;
  output_tokens: number;
  cache_read_tokens: number;
  cache_write_tokens: number;
  reasoning_tokens: number;
}

export interface ReceiptFeaturesView {
  estimated_input_tokens: number;
  message_count: number;
  tool_count: number;
  has_images: boolean;
  requires_json_schema: boolean;
  code_block_count: number;
  requested_max_output_tokens: number | null;
  hint_count: number;
  reasoning_marker_count: number;
  technical_term_count: number;
  simple_indicator_count: number;
  code_keyword_count: number;
  math_term_count: number;
  creative_term_count: number;
  multi_step_signal: number;
  question_count: number;
  system_format_hint: boolean;
}

export type ReceiptDecidedByView =
  | { tier: "rule"; rule: string }
  | { tier: "hint"; kind: "step_type" | "task_type" | "preference" | "capability"; value: string }
  | { tier: "heuristic"; score: number; threshold: number }
  | { tier: "default" }
  | { tier: "exact_model"; model: string };

export interface ReceiptRouteView {
  upstream: string;
  model: string;
  pool: string;
  decided_by: ReceiptDecidedByView;
  fallbacks: number;
  features: ReceiptFeaturesView;
}

export interface ReceiptAttemptView {
  ordinal: number;
  upstream: string;
  model: string;
  latency_ms: number;
  http_status: number | null;
  error_code: ReceiptErrorCode | null;
  stream_outcome: "complete" | "failed_after_partial" | "failed_before_output" | "client_cancelled" | null;
  fallback_allowed: boolean;
}

export interface ReceiptConversionView {
  ordinal: number;
  stage: "inbound_normalize" | "provider_request" | "provider_response" | "outbound_render" | "stream_translate";
  source_protocol: string;
  target_protocol: string;
  succeeded: boolean;
  error_code: ReceiptErrorCode | null;
}

export interface ReceiptView {
  request_id: string;
  started_at_ms: number;
  latency_ms: number;
  protocol: string;
  requested_model: string;
  stream: boolean;
  status: number;
  error_code: ReceiptErrorCode | null;
  attempts: number;
  routing: ReceiptRouteView | null;
  usage: ReceiptUsageView | null;
  cost_micros: number | null;
  price_version: number | null;
  agent_id: string | null;
  running_revision: number | null;
  cost_kind: ReceiptCostKind;
  decision: ReceiptRouteView | null;
  attempt_records: ReceiptAttemptView[];
  conversion_reports: ReceiptConversionView[];
}

export type ServePhase = "stopped" | "starting" | "stopping" | "running" | "error";

export interface ServeView {
  phase: ServePhase;
  app_runtime: "stopped" | "running";
  listener_reachable: boolean;
  agent_connected: boolean;
  running_revision: number | null;
  instance_id: string | null;
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
  deleted_providers?: string[];
  provider_recovery_error?: string | null;
  tiers: Record<TierSlot, TierView>;
  agent_routes: Record<string, AgentRouteView>;
  profiles: string[];
  serve: ServeView;
  draft_revision: number;
  saved_revision: number;
  config_dirty: boolean;
  config_error: string | null;
  settings: SettingsView;
}

export type AgentId = string;
export type AgentAdmission = "supported" | "discovery_only";
export type AgentPlatform = "macos" | "linux" | "windows" | "wsl";
export type AgentStatus =
  | "NOT_DETECTED"
  | "DETECTED_VERIFIED"
  | "DETECTED_UNKNOWN"
  | "DETECTED_BLOCKED"
  | "INSTALLED_BROKEN"
  | "MULTIPLE_INSTALLATIONS"
  | "CONNECTED";
export type AgentPlanIntent = "connect" | "disconnect" | "restore";
export type AgentConfirmationKind =
  | "installation"
  | "target_config"
  | "configuration_diff";

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
  catalog_source: "builtin";
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

export const previewProviderEndpoints = (base_url: string) =>
  invoke<ProviderEndpointPreview>("preview_provider_endpoints", { baseUrl: base_url });

export const addProvider = (
  name: string,
  base_url: string,
  models: string[],
  api_key: string | null,
) => invoke<StateView>("add_provider", { name, baseUrl: base_url, models, apiKey: api_key });

export const editProvider = (name: string, base_url: string, api_key: string | null) =>
  invoke<StateView>("edit_provider", { name, baseUrl: base_url, apiKey: api_key });

export const removeProvider = (name: string) =>
  invoke<StateView>("remove_provider", { name });

export const previewProviderRemoval = (name: string) =>
  invoke<ProviderRemovalPreview>("preview_provider_removal", { name });

export const restoreProvider = (name: string) =>
  invoke<StateView>("restore_provider", { name });

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

export const testProvider = (name: string) =>
  invoke<ProviderTestResult[]>("test_provider", { name });

export const updateProviderModels = (name: string, models: string[]) =>
  invoke<StateView>("update_provider_models", { name, models });

export const setTier = (
  slot: TierSlot,
  upstream: string | null,
  model: string | null,
) => invoke<StateView>("set_tier", { slot, upstream, model });

export const setAgentRouteMode = (agentId: AgentId, mode: AgentRouteMode) =>
  invoke<StateView>("set_agent_route_mode", { agentId, mode });

export const setAgentTier = (
  agentId: AgentId,
  slot: TierSlot,
  upstream: string | null,
  model: string | null,
) => invoke<StateView>("set_agent_tier", { agentId, slot, upstream, model });

export const saveHomeRouteAsProfile = (name: string) =>
  invoke<StateView>("save_home_route_as_profile", { name });

export const mountAgentProfile = (agentId: AgentId, profile: string) =>
  invoke<StateView>("mount_agent_profile", { agentId, profile });

export const deleteProfile = (name: string) =>
  invoke<StateView>("delete_profile", { name });

export const saveAgentRoutes = () => invoke<StateView>("save_agent_routes");

export const applyHomeRouteToAllAgents = () =>
  invoke<StateView>("apply_home_route_to_all_agents");

export const saveConfig = () => invoke<StateView>("save_config");

export const getRuntimeState = () => invoke<ServeView>("get_runtime_state");
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
) => invoke<AgentOperationView>("apply_agent_plan", {
  operationId,
  confirmationToken,
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
  const reachable = serve.app_runtime === "running" && serve.listener_reachable;
  adminBase = reachable ? `http://${serve.listen}` : null;
  adminKey = reachable ? serve.virtual_key : null;
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

export const getRecentReceipts = (limit = 5) => {
  const bounded = Math.min(5, Math.max(1, limit));
  return dataGet<ReceiptView[]>("/admin/receipts", () =>
    invoke<ReceiptView[]>("get_recent_receipts", { limit: bounded }),
  );
};

// 注意语义差:HTTP 返回**运行中**配置的路由表,IPC 回退返回可编辑草稿。
// 代理运行时以运行态为准,正是数据面该报告的事实。
export const getRouterTable = () =>
  dataGet<RouterTableView>("/admin/router-table", () =>
    invoke<RouterTableView>("get_router_table"),
  );

export const getPlugins = () =>
  dataGet<PluginsView>("/admin/plugins", () => invoke<PluginsView>("get_plugins"));

export const checkUpgrade = () => invoke<UpgradeView>("check_upgrade");
