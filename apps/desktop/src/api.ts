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

export type AgentRouteMode = "inherit" | "custom";

export interface AgentRouteView {
  mode: AgentRouteMode;
  tiers: Record<TierSlot, TierView>;
  config_error: string | null;
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

export const getState = () => invoke<StateView>("get_state");

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

export const planAgentConnection = (agentId: AgentId, installationPath: string) =>
  invoke<ConfigPlanView>("plan_agent_connection", { agentId, installationPath });

export const applyAgentPlan = (operationId: string, confirmationToken: string) =>
  invoke<AgentOperationView>("apply_agent_plan", { operationId, confirmationToken });

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
// Read-only data plane: prefer local HTTP `/admin/*` so the same frontend can run without the Tauri shell
// (direct browser development and a future remote console). In the Tauri shell, IPC is the fallback if the proxy is stopped or the request fails
// IPC keeps the usage and routing-table pages available when the proxy is stopped. It reads drafts and the local database. This matches
// Behavior matches the state before the change. Privileged operations, including Agent transactions, configuration writes, and secrets, use IPC only and never HTTP.

const IN_TAURI = "__TAURI_INTERNALS__" in window;

let adminBase: string | null = null;
let adminKey: string | null = null;

/** Synchronize the data-plane endpoint whenever App.tsx refreshes state. */
export function setAdminEndpoint(serve: ServeView) {
  adminBase = serve.phase === "running" ? `http://${serve.listen}` : null;
  adminKey = serve.phase === "running" ? serve.virtual_key : null;
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

export const getStats = (since: string, by: string | null) =>
  dataGet<StatsView>(
    `/admin/stats?since=${since}${by ? `&by=${by}` : ""}`,
    () => invoke<StatsView>("get_stats", { since, by }),
  );

// Note the semantic difference: HTTP returns the active routing table. IPC fallback returns the editable draft.
// Use runtime state while the proxy runs. This is the correct data-plane fact.
export const getRouterTable = () =>
  dataGet<RouterTableView>("/admin/router-table", () =>
    invoke<RouterTableView>("get_router_table"),
  );

export const getPlugins = () =>
  dataGet<PluginsView>("/admin/plugins", () => invoke<PluginsView>("get_plugins"));

export const checkUpgrade = () => invoke<UpgradeView>("check_upgrade");
