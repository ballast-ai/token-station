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
  /** Locally hosted provider, such as Ollama. Local-only routing uses this to keep traffic on the machine. */
  local?: boolean;
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
  duration_ms?: number;
  timing_kind?: "cumulative" | "stage";
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
  capabilities_updated?: boolean;
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

export interface ReceiptPageView {
  items: ReceiptView[];
  total: number;
  page: number;
  page_size: number;
}

export interface ReceiptPageQuery {
  since: string;
  agentId?: string | null;
  upstream?: string | null;
  model?: string | null;
  status?: "success" | "error" | null;
  page: number;
  pageSize?: number;
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
  egress_mode: "direct" | "http" | "socks5";
  egress_proxy_url: string;
  egress_no_proxy: string[];
  egress_auth_username: string;
  egress_auth_slot: string;
}

export interface EgressView {
  mode: "direct" | "http" | "socks5";
  proxy_url: string | null;
  no_proxy: string[];
  auth_slot: string | null;
  routes: Array<{
    request_class: "provider_request" | "model_catalog" | "health_probe";
    upstream: string;
    target: string;
    route: "direct" | "proxy";
    matched_no_proxy: boolean;
  }>;
  fixed_direct_classes: string[];
}

export interface StateView {
  providers: ProviderView[];
  deleted_providers?: string[];
  provider_recovery_error?: string | null;
  tiers: Record<TierSlot, TierView>;
  /** User keyword library for each tier; a match forces that tier at routing layer 1. */
  keywords: Record<TierSlot, string[]>;
  agent_routes: Record<string, AgentRouteView>;
  profiles: string[];
  /** Local-only routing uses providers marked local and keeps requests on the machine. */
  local_only: boolean;
  /** Whether local_only may fall back to cloud when no local target is available; false means strict local routing. */
  allow_cloud_fallback: boolean;
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
  ui_order?: number;
  nav_mark?: string;
  connector_capabilities?: Array<{
    connector_id: string;
    adapter_id: string;
    base_url_shape: "origin" | "origin_v1";
    platforms: Array<"macos" | "linux" | "windows" | "wsl">;
    config_format: string;
    config_path_template: string;
    owned_fields: string[];
    requires_virtual_key: boolean;
    restart_required: boolean;
  }>;
}

export interface AgentDiagnosticView {
  reason_code: string;
  message: string;
}

export interface AgentDiscoveryView {
  agent_id: AgentId;
  executable_path: string;
  canonical_path: string;
  binary_source: "homebrew" | "npm_global" | "path" | "known_path" | "env_override";
  modified_at_ms: number | null;
  binary_sha256: string | null;
  upgrade_command: string | null;
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
  managed: boolean;
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

export type DriftStatus =
  | "unmanaged"
  | "in_sync"
  | "unowned_changes"
  | "managed_changes"
  | "missing"
  | "unreadable"
  | "unparseable";

export interface AgentDriftView {
  agent_id: AgentId;
  installation_path: string;
  target_config_path: string;
  connector_id: string;
  status: DriftStatus;
  baseline_hash: string;
  managed_hash: string;
  current_hash: string | null;
  checked_at_ms: number;
  changes: Array<{
    path: { segments: string[] };
    scope: "managed" | "unowned";
    kind: "added" | "removed" | "changed";
    current_matches_managed: boolean | null;
  }>;
  truncated: boolean;
  message: string;
}

export interface ConfigPlanView {
  schema_version: number;
  operation_id: string;
  intent: AgentPlanIntent;
  agent_id: AgentId;
  installation_path: string;
  target_config_path: string;
  related_config_paths?: string[];
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
  projection: {
    schema_version: number;
    files: Array<{
      target_config_path: string;
      format: string;
      target_existed: boolean;
      before_hash: string;
      expected_after_hash: string;
      owned_paths: Array<{ segments: string[] }>;
      forward_changes: ConfigPlanView["changes"];
      reverse_changes: ConfigPlanView["changes"];
      credential_bindings: Array<{
        path: { segments: string[] };
        source: "local_virtual_key" | "encrypted_snapshot";
      }>;
    }>;
  };
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
  cache_read_tokens: number;
  cache_write_tokens: number;
  reasoning_tokens: number;
  cost_micros: number | null;
  priced_requests: number;
  unpriced_requests: number;
}

export interface StatsView {
  total: AggView;
  groups: [string, AggView][];
  by: string | null;
  empty: boolean;
}

export type BudgetUsageLevel = "healthy" | "approaching" | "exceeded" | "unknown";
export type BudgetExpiryLevel = "none" | "active" | "expiring" | "expired";

export interface BudgetStatus {
  agent_id: AgentId;
  limit_micros: number;
  used_micros: number;
  remaining_micros: number;
  warning_percent: number;
  usage_percent: number;
  unpriced_requests: number;
  period_start_ms: number | null;
  period_end_ms: number | null;
  expiry_warning_days: number;
  usage_level: BudgetUsageLevel;
  expiry_level: BudgetExpiryLevel;
  enforcement: "observe_only";
  routing_affected: false;
}

export interface ModelPriceView {
  input_per_mtok: number;
  output_per_mtok: number;
  cache_read_per_mtok: number;
  cache_write_per_mtok: number;
  reasoning_per_mtok: number | null;
}

export interface PriceTableView {
  version: number;
  models: Record<string, ModelPriceView>;
}

export interface ModelPriceSuggestionView extends ModelPriceView {
  model_id: string;
  display_name: string;
  provider_id: string;
  provider_name: string;
  source: "models.dev";
  catalog_source: "live" | "cache" | "stale_cache";
  fetched_at_ms: number;
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
  // Pass rules and hint_routes through unchanged; use loose types as the core schema evolves.
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

export interface RecoveryState {
  mode: "normal" | "safe";
  reason_code: "metrics_schema_newer" | "metrics_unreadable" | null;
  message: string | null;
  found_schema: number | null;
  supported_schema: number | null;
  metrics_path: string;
  backup_dir: string;
  local_only: boolean;
}

export interface FrontendDiagnosticInput {
  kind: "render_error" | "window_error" | "unhandled_rejection" | "runtime_error";
  message: string;
  stack: string | null;
  component_stack: string | null;
}

export interface FrontendDiagnosticRecord extends FrontendDiagnosticInput {
  timestamp_ms: number;
}

export interface DiagnosticPreview {
  recovery: RecoveryState;
  frontend_events: FrontendDiagnosticRecord[];
  export_includes: string[];
  local_only: boolean;
  redacted: boolean;
  auto_upload: boolean;
}

export const getState = () => invoke<StateView>("get_state");
export const getRecoveryState = () => invoke<RecoveryState>("get_recovery_state");
export const getRecoveryDiagnostics = () =>
  invoke<DiagnosticPreview>("get_recovery_diagnostics");
export const recordFrontendDiagnostic = (event: FrontendDiagnosticInput) =>
  invoke<FrontendDiagnosticRecord>("record_frontend_diagnostic", { event });
export const exportRecoveryBundle = (confirmed: boolean) =>
  invoke<string>("export_recovery_bundle", { confirmed });
export const openRecoveryFolder = () => invoke<string>("open_recovery_folder");

export const previewProviderEndpoints = (base_url: string) =>
  invoke<ProviderEndpointPreview>("preview_provider_endpoints", { baseUrl: base_url });

export const addProvider = (
  name: string,
  base_url: string,
  models: string[],
  api_key: string | null,
  local = false,
) =>
  invoke<StateView>("add_provider", {
    name,
    baseUrl: base_url,
    models,
    apiKey: api_key,
    local,
  });

/** Set local-only routing and cloud fallback in the home router; inherited Agents follow automatically. */
export const setLocalRouting = (localOnly: boolean, allowCloudFallback: boolean) =>
  invoke<StateView>("set_local_routing", {
    localOnly,
    allowCloudFallback,
  });

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

export const setProviderModelVision = (name: string, model: string, supported: boolean) =>
  invoke<StateView>("set_provider_model_vision", { name, model, supported });

export const updateProviderModels = (name: string, models: string[]) =>
  invoke<StateView>("update_provider_models", { name, models });

export const setTier = (
  slot: TierSlot,
  upstream: string | null,
  model: string | null,
) => invoke<StateView>("set_tier", { slot, upstream, model });

/** Add a keyword to a tier; matching it forces that tier. */
export const addKeyword = (slot: TierSlot, keyword: string) =>
  invoke<StateView>("add_keyword", { slot, keyword });

/** Remove a keyword from a tier. */
export const removeKeyword = (slot: TierSlot, keyword: string) =>
  invoke<StateView>("remove_keyword", { slot, keyword });

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

export const getAgentBudgets = () =>
  invoke<BudgetStatus[]>("get_agent_budgets");

export const setAgentBudget = (
  agentId: AgentId,
  limitMicros: number,
  warningPercent: number,
  periodStartMs: number | null,
  periodEndMs: number | null,
  expiryWarningDays: number,
) => invoke<BudgetStatus[]>("set_agent_budget", {
  agentId,
  limitMicros,
  warningPercent,
  periodStartMs,
  periodEndMs,
  expiryWarningDays,
});

export const removeAgentBudget = (agentId: AgentId) =>
  invoke<BudgetStatus[]>("remove_agent_budget", { agentId });

export const getPriceTable = () => invoke<PriceTableView>("get_price_table");

export const suggestModelPrice = (providerId: string | null, modelId: string) =>
  invoke<ModelPriceSuggestionView | null>("suggest_model_price", {
    providerId,
    modelId,
  });

export const setModelPrice = (
  model: string,
  price: ModelPriceView,
  expectedVersion: number,
) => invoke<PriceTableView>("set_model_price", {
  model,
  inputPerMtok: price.input_per_mtok,
  outputPerMtok: price.output_per_mtok,
  cacheReadPerMtok: price.cache_read_per_mtok,
  cacheWritePerMtok: price.cache_write_per_mtok,
  reasoningPerMtok: price.reasoning_per_mtok,
  expectedVersion,
});

export const removeModelPrice = (model: string, expectedVersion: number) =>
  invoke<PriceTableView>("remove_model_price", { model, expectedVersion });

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

/** Forced-disconnect fallback: if a lost key prevents snapshot decryption and recovery fails, remove managed fields and clear ownership. */
export const forceForgetAgent = (agentId: AgentId, installationPath: string) =>
  invoke<void>("force_forget_agent", { agentId, installationPath });

export const listAgentSnapshots = (agentId: AgentId) =>
  invoke<SnapshotView[]>("list_agent_snapshots", { agentId });

export const getAgentDrift = (agentId: AgentId, installationPath: string) =>
  invoke<AgentDriftView[]>("get_agent_drift", { agentId, installationPath });

export const planSnapshotRestore = (snapshotId: string) =>
  invoke<ConfigPlanView>("plan_snapshot_restore", { snapshotId });

export const applySnapshotRestore = (operationId: string, confirmationToken: string) =>
  invoke<AgentOperationView>("apply_snapshot_restore", {
    operationId,
    confirmationToken,
  });

export const setSettings = (
  auth: boolean,
  metrics: boolean,
  egress: Pick<SettingsView, "egress_mode" | "egress_proxy_url" | "egress_no_proxy" | "egress_auth_username" | "egress_auth_slot">,
) => invoke<StateView>("set_settings", {
  auth,
  metrics,
  egressMode: egress.egress_mode,
  egressProxyUrl: egress.egress_proxy_url,
  egressNoProxy: egress.egress_no_proxy,
  egressAuthUsername: egress.egress_auth_username,
  egressAuthSlot: egress.egress_auth_slot,
});

// ---------------------------------------------------------------------------
// Read-only data plane: prefer local HTTP `/admin/*` so the same frontend can run without the Tauri shell
// (direct browser development and a future remote console). In the Tauri shell, IPC is the fallback if the proxy is stopped or the request fails
// IPC keeps the usage and routing-table pages available when the proxy is stopped. It reads drafts and the local database. This matches
// Behavior matches the state before the change. Privileged operations, including Agent transactions, configuration writes, and secrets, use IPC only and never HTTP.

const IN_TAURI = "__TAURI_INTERNALS__" in window;

let adminBase: string | null = null;
let adminKey: string | null = null;

/** Synchronize the data-plane endpoint whenever App.tsx refreshes state. */
export function setAdminEndpoint(serve: ServeView) {
  const reachable = serve.app_runtime === "running" && serve.listener_reachable;
  adminBase = reachable ? `http://${serve.listen}` : null;
  adminKey = reachable ? serve.virtual_key : null;
}

// Browser-only mode without a Tauri shell cannot call get_state. Read the endpoint from localStorage,
// Default local host and port. Usage: localStorage.setItem("ts_listen","127.0.0.1:8787");
// Refresh the page after `localStorage.setItem("ts_key","<virtual key>")`.
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
      // Fall back to IPC for non-2xx responses such as an invalid key; browser-only mode throws.
    } catch {
      // Fall back after network failures, such as when the proxy has just stopped.
    }
  }
  if (IN_TAURI) return ipcFallback();
  throw new Error(
    "无法连接本地代理:请确认 token-station serve 已启动,并在 localStorage 配置 ts_listen / ts_key",
  );
}

export const getStats = (
  since: string,
  by: string | null,
  agentId: string | null = null,
  source: string | null = null,
  upstream: string | null = null,
  model: string | null = null,
) =>
  dataGet<StatsView>(
    `/admin/stats?since=${encodeURIComponent(since)}`
      + `${by ? `&by=${encodeURIComponent(by)}` : ""}`
      + `${agentId ? `&agent=${encodeURIComponent(agentId)}` : ""}`
      + `${source ? `&source=${encodeURIComponent(source)}` : ""}`
      + `${upstream ? `&upstream=${encodeURIComponent(upstream)}` : ""}`
      + `${model ? `&model=${encodeURIComponent(model)}` : ""}`,
    () => invoke<StatsView>("get_stats", {
      since,
      by,
      agentId,
      source,
      upstream,
      model,
    }),
  );

export const getRecentReceipts = (limit = 5) => {
  const bounded = Math.min(5, Math.max(1, limit));
  return dataGet<ReceiptView[]>("/admin/receipts", () =>
    invoke<ReceiptView[]>("get_recent_receipts", { limit: bounded }),
  );
};

export const getRequestReceipts = ({
  since,
  agentId = null,
  upstream = null,
  model = null,
  status = null,
  page,
  pageSize = 20,
}: ReceiptPageQuery) =>
  invoke<ReceiptPageView>("get_request_receipts", {
    since,
    agentId,
    upstream,
    model,
    status,
    page,
    pageSize,
  });

// Note the semantic difference: HTTP returns the active routing table. IPC fallback returns the editable draft.
// Use runtime state while the proxy runs. This is the correct data-plane fact.
export const getRouterTable = () =>
  dataGet<RouterTableView>("/admin/router-table", () =>
    invoke<RouterTableView>("get_router_table"),
  );

export const getPlugins = () =>
  dataGet<PluginsView>("/admin/plugins", () => invoke<PluginsView>("get_plugins"));

export const getEgress = () =>
  dataGet<EgressView>("/admin/egress", () => invoke<EgressView>("get_egress"));

export const checkUpgrade = () => invoke<UpgradeView>("check_upgrade");
