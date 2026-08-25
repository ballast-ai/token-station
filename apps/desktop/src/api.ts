import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export interface TierView {
  upstream: string | null;
  model: string | null;
}

export interface ProviderView {
  name: string;
  /** Stable catalog brand identifier used for local provider artwork. */
  brand_id?: string | null;
  provider: string;
  base_url: string;
  models: string[];
  model_capabilities?: ModelCapabilityView[];
  catalog_revision?: number;
  catalog?: CatalogModelView[];
  has_auth: boolean;
  credential_source?: "store" | "env" | "file" | "none";
  credential_reference?: string;
  provider_call?: ProviderCallEngine;
  south_v1_available?: boolean;
  south_v1_unavailable_reason?: SouthUnavailableReason | null;
  south_header_auth_v1_available?: boolean;
  south_header_auth_v1_unavailable_reason?: SouthUnavailableReason | null;
  /** Locally hosted provider, such as Ollama. Local-only routing uses this to keep traffic on the machine. */
  local?: boolean;
  access_tier?: "free" | "paid";
  /** Declared quota plan for local estimates; absent means non-windowed or usage-based. */
  quota_plan?: QuotaPlanView | null;
}

export type ProviderCallEngine =
  | "legacy"
  | "south_v1_buffered"
  | "south_v1_buffered_streaming"
  | "south_v1_buffered_streaming_header_auth";

export type SouthUnavailableReason = "provider_package" | "api_dialect" | "egress" | "auth";

export interface QuotaPlanView {
  len_ms: number;
  limit: number;
  unit: "tokens" | "requests";
  rate_limit_per_min: number | null;
}

export type CapabilityState = "verified" | "declared" | "unsupported" | "unknown";

export type FreeOfferKind = "recurring" | "trial";
export type FreeProviderRegion = "china" | "global";
export type FreeOveragePolicy = "hard_stop" | "rate_limited" | "user_must_enable_guard";

export interface FreeModelPresetView {
  id: string;
  label: string;
  tool: CapabilityState;
  vision: CapabilityState;
  json_schema: CapabilityState;
  context_window: number;
}

export interface FreeProviderPresetView {
  id: string;
  upstream_name: string;
  label: string;
  short_label: string;
  base_url: string;
  offer_kind: FreeOfferKind;
  region: FreeProviderRegion;
  tags: string[];
  free_note: string;
  key_instruction: string;
  application_url: string;
  docs_url: string;
  verified_at: string;
  overage_policy: FreeOveragePolicy;
  models: FreeModelPresetView[];
}

export interface ModelCapabilityView {
  model: string;
  tool: CapabilityState;
  vision: CapabilityState;
  json_schema: CapabilityState;
  context_window?: number | null;
  max_output_tokens?: number | null;
  context_window_source?: ModelLimitSource;
  max_output_tokens_source?: ModelLimitSource;
}

export type ModelLimitSource = "provider" | "builtin_preset" | "operator" | "heuristic";

export type CatalogSource = "live" | "cache" | "configured";
export type CatalogState = "active" | "stale" | "removed";

export interface CatalogModelView extends ModelCapabilityView {
  context_window?: number | null;
  max_output_tokens?: number | null;
  cost?: {
    input?: number | null;
    output?: number | null;
    cache_read?: number | null;
    cache_write?: number | null;
  } | null;
  source: CatalogSource;
  last_seen_ms: number | null;
  catalog_state: CatalogState;
}

export interface ProviderEndpointPreview {
  chat: string;
  responses: string;
  messages: string;
  /** Backend-determined loopback eligibility; only true endpoints can be marked as local models. */
  loopback: boolean;
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

export interface ModelTestMessage {
  role: "user" | "assistant";
  content: string;
}

export interface ModelTestReply {
  content: string;
  first_token_ms: number;
  latency_ms: number;
}

export interface ModelTestStreamEvent {
  request_id: string;
  delta: string;
  first_token_ms: number | null;
}

export interface ModelDiscoveryView {
  models: string[];
  source: "live" | "cache" | "preset" | "none";
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
  | { tier: "heuristic"; score: number; matched_band_at_least: number }
  | { tier: "default" }
  | { tier: "exact_model"; model: string }
  | { tier: "quota" };

/** Quota-first decision snapshot explaining the selected account's window and rate state. */
export interface ReceiptQuotaView {
  reset_ms: number | null;
  remaining_permille: number | null;
  headroom_permille: number;
  pressured: boolean;
  exhausted: boolean;
}

export interface ReceiptRouteView {
  upstream: string;
  model: string;
  pool: string;
  decided_by: ReceiptDecidedByView;
  fallbacks: number;
  features: ReceiptFeaturesView;
  /** Present only for quota-first routing; undefined for tiered routing. */
  quota?: ReceiptQuotaView | null;
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
  outcome?: "succeeded" | "failed" | "cancelled" | "unknown";
  error_code: ReceiptErrorCode | null;
  reason_code?:
    | "unsupported_tool_type"
    | "provider_tool_unsupported"
    | "stateful_chaining"
    | "structured_output"
    | "reasoning_item"
    | "unsupported_media"
    | "invalid_json"
    | "invalid_protocol_shape"
    | "adapter_failure"
    | null;
  reason_detail?:
    | "local_shell"
    | "web_search"
    | "function_tool"
    | "previous_response_id"
    | "json_schema"
    | "reasoning"
    | "image"
    | "request_body"
    | "other_tool_type"
    | null;
}

export interface ReceiptView {
  request_id: string;
  started_at_ms: number;
  latency_ms: number;
  protocol: string;
  request_method?: string | null;
  path_kind?:
    | "chat_completions"
    | "responses"
    | "messages"
    | "gemini_generate_content"
    | "models"
    | "embeddings"
    | "admin"
    | "unknown_agent_endpoint"
    | "unknown";
  requested_model: string;
  stream: boolean;
  status: number;
  error_code: ReceiptErrorCode | null;
  attempts: number;
  routing: ReceiptRouteView | null;
  usage: ReceiptUsageView | null;
  usage_semantics?: "provider_reported_v1" | "canonical_total_v2";
  cost_micros: number | null;
  price_version: number | null;
  agent_id: string | null;
  running_revision: number | null;
  cost_kind: ReceiptCostKind;
  decision: ReceiptRouteView | null;
  attempt_records: ReceiptAttemptView[];
  conversion_reports: ReceiptConversionView[];
}

export interface RequestPlaintextView {
  request_id: string;
  captured_at_ms: number;
  input: string;
  output: string;
  input_truncated: boolean;
  output_truncated: boolean;
}

export interface ReceiptPageView {
  items: ReceiptView[];
  plaintext_by_request_id?: Record<string, RequestPlaintextView>;
  plaintext_errors_by_request_id?: Record<string, string>;
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
  /** True when Home model tests share this live Gateway's mutable state. */
  model_test_uses_running_gateway: boolean;
}

export type TierSlot = "high" | "mid" | "low";

export type RoutingMode = "direct" | "tiered" | "quota_first";

export interface DirectRouteTarget {
  upstream: string;
  /** Null preserves a known provider when its previously selected model was removed. */
  model: string | null;
}

export type AgentRouteMode = "inherit" | "custom" | "profile";

export interface AgentRouteView {
  mode: AgentRouteMode;
  /** True when no per-Agent routing axis overrides the global configuration. */
  inherits_global?: boolean;
  tiers: Record<TierSlot, TierView>;
  config_error: string | null;
  profile: string | null;
  /** Effective routing mode for this Agent: its override first, otherwise the home default. */
  routing_mode: RoutingMode;
  /** Effective exact target for Direct mode; absent/null means configuration is incomplete. */
  direct_target?: DirectRouteTarget | null;
}

export interface QuotaAccount {
  upstream: string;
  model: string;
}

/** Runtime quota source: authoritative provider headers, local ledger estimate, or no data. */
export type QuotaSource = "authoritative" | "estimated" | "none";

export interface QuotaWindowSnapshot {
  len_ms: number;
  limit: number;
  used: number;
  remaining_permille: number;
  ms_until_reset: number;
}

export interface QuotaAccountSnapshot {
  upstream: string;
  windows: QuotaWindowSnapshot[];
  rate_headroom_permille: number;
  rate_pressured: boolean;
  inflight: number;
  exhausted: boolean;
  cooling_ms_remaining: number;
  source: QuotaSource;
}

export interface QuotaSnapshot {
  now_ms: number;
  accounts: QuotaAccountSnapshot[];
}

export interface SettingsView {
  listen: string;
  auth: boolean;
  metrics: boolean;
  data_dir: string;
  plugins_dir: string;
  agent: string;
  version: string;
  desktop_version?: string;
  core_version?: string;
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
  /** Routing mode: an exact target, intelligent tiers, or quota-first rotation. */
  routing_mode: RoutingMode;
  /** Exact Home target used by Direct mode; null means the draft is incomplete. */
  direct_target?: DirectRouteTarget | null;
  /** Globally shared quota-first rotation accounts, provider plus model, in priority order. */
  quota_accounts: QuotaAccount[];
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
  binary_source: "homebrew" | "npm_global" | "microsoft_store" | "path" | "known_path" | "env_override";
  modified_at_ms: number | null;
  binary_sha256: string | null;
  upgrade_command: string | null;
  version_raw: string | null;
  version_normalized: string | null;
  environment: AgentPlatform;
  evidence: Array<{
    source: "known_path" | "package_manager" | "path" | "env_override";
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
  adapter_ready: boolean | null;
  connection_issue?: {
    code: string;
    message: string;
    target?: string | null;
  } | null;
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
  legacy_input_requests: number;
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

export interface PublicProviderModelsView {
  providers: Record<string, string[]>;
  source: "live" | "cache" | "stale_cache";
  fetched_at_ms: number;
  unavailable_provider_ids: string[];
}

export interface ModelPriceImportResultView {
  state: StateView;
  imported: number;
  existing: number;
  missing_model_ids: string[];
  price_version: number;
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

export type DesktopUpdateStatus =
  | "up_to_date"
  | "update_available"
  | "unsupported"
  | "unavailable";

export interface DesktopUpdateView {
  status: DesktopUpdateStatus;
  current_version: string;
  version: string | null;
  notes: string | null;
  pub_date: string | null;
  release_url: string;
  message: string | null;
}

export interface DesktopUpdateProgress {
  downloaded: number;
  total: number | null;
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
export const setDockThemeIcon = (theme: "light" | "dark") =>
  invoke<void>("set_dock_theme_icon", { theme });
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
  credential_source: "store" | "env" | "file" | "none" = api_key ? "store" : "none",
  credential_reference: string | null = null,
  provider_dialect: "openai-compatible" | "azure-openai-v1" = "openai-compatible",
) =>
  invoke<StateView>("add_provider_with_credential", {
    name,
    baseUrl: base_url,
    models,
    apiKey: api_key,
    local,
    credentialSource: credential_source,
    credentialReference: credential_reference,
    providerDialect: provider_dialect,
  });

export const addManagedEnterpriseRoute = (
  name: string,
  base_url: string,
  api_key: string,
) => invoke<StateView>("add_managed_enterprise_route", {
  name,
  baseUrl: base_url,
  apiKey: api_key,
});

export const listFreeProviderPresets = () =>
  invoke<FreeProviderPresetView[]>("list_free_provider_presets");

export const addFreeProvider = (
  presetId: string,
  selectedModels: string[],
  apiKey: string,
  guardConfirmed: boolean,
) =>
  invoke<StateView>("add_free_provider", {
    presetId,
    selectedModels,
    apiKey,
    guardConfirmed,
  });

/** Set local-only routing and cloud fallback in the home router; inherited Agents follow automatically. */
export const setLocalRouting = (localOnly: boolean, allowCloudFallback: boolean) =>
  invoke<StateView>("set_local_routing", {
    localOnly,
    allowCloudFallback,
  });

/** Switch among exact-target, tiered intelligent, and quota-first routing. */
export const setRoutingMode = (mode: RoutingMode, agentId?: string) =>
  invoke<StateView>("set_routing_mode", { mode, agentId: agentId ?? null });

/** Persist one exact Provider/model target; display order is intentionally not part of this command. */
export const setDirectRoute = (upstream: string, model: string, agentId?: string) =>
  invoke<StateView>("set_direct_route", {
    upstream,
    model,
    agentId: agentId ?? null,
  });

export const setQuotaAccounts = (accounts: QuotaAccount[]) =>
  invoke<StateView>("set_quota_accounts", { accounts });

/** Query the running gateway's live quota snapshot; requires the proxy to be running. */
export const getQuotaSnapshot = () => invoke<QuotaSnapshot>("get_quota_snapshot");

/** Declare or clear a provider quota plan for local estimates; zero limit or len_ms clears it. */
export const setQuotaPlan = (
  upstream: string,
  lenMs: number,
  limit: number,
  unit: "tokens" | "requests",
  rateLimitPerMin: number | null,
) =>
  invoke<StateView>("set_quota_plan", {
    upstream,
    lenMs,
    limit,
    unit,
    rateLimitPerMin,
  });

/**
 * Edits a provider's endpoint and credential. The transport engine is not
 * an editable detail: South is the default and the host decides per call
 * whether an attempt can use it, so this never sends `providerCall`.
 */
export const editProvider = (
  name: string,
  base_url: string,
  api_key: string | null,
  credential_source?: "store" | "env" | "file" | "none",
  credential_reference?: string | null,
) => credential_source
  ? invoke<StateView>("edit_provider_with_credential", {
    name,
    baseUrl: base_url,
    apiKey: api_key,
    credentialSource: credential_source,
    credentialReference: credential_reference ?? null,
  })
  : invoke<StateView>("edit_provider", { name, baseUrl: base_url, apiKey: api_key });

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

export const verifyEnterpriseRoute = (
  name: string,
  base_url: string,
  api_key: string,
) => invoke<ModelDiscoveryView>("verify_enterprise_route", {
  name,
  baseUrl: base_url,
  apiKey: api_key,
});

export const testProvider = (name: string) =>
  invoke<ProviderTestResult[]>("test_provider", { name });

export const testModelChatStream = async (
  messages: ModelTestMessage[],
  requestId: string,
  onDelta: (event: ModelTestStreamEvent) => void,
) => {
  const unlisten = await listen<ModelTestStreamEvent>("model-test-stream", (event) => {
    if (event.payload.request_id === requestId) onDelta(event.payload);
  });
  try {
    return await invoke<ModelTestReply>("test_model_chat_stream", {
      messages,
      requestId,
    });
  } finally {
    unlisten();
  }
};

export const cancelModelTestChat = (requestId: string) =>
  invoke<void>("cancel_model_test_chat", { requestId });

export const setProviderModelVision = (name: string, model: string, supported: boolean) =>
  invoke<StateView>("set_provider_model_vision", { name, model, supported });

export const setProviderModelLimits = (
  name: string,
  model: string,
  contextWindow: number,
  maxOutputTokens: number,
) => invoke<StateView>("set_provider_model_limits", {
  name,
  model,
  contextWindow,
  maxOutputTokens,
});

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

/** Save and hot-restart one Agent route; apply immediately without affecting other Agents. */
export const restartAgentRoute = (agentId: string) =>
  invoke<StateView>("restart_agent_route", { agentId });

export const applyHomeRouteToAllAgents = () =>
  invoke<StateView>("apply_home_route_to_all_agents");

export const saveConfig = () => invoke<StateView>("save_config");

export const getRuntimeState = () => invoke<ServeView>("get_runtime_state");
export const ensureServeRunning = () => invoke<StateView>("ensure_serve_running");
export const serveStart = () => invoke<StateView>("serve_start");
export const serveStop = () => invoke<StateView>("serve_stop");

export const listenServeState = (handler: (serve: ServeView) => void) =>
  listen<ServeView>("serve-state-changed", (event) => handler(event.payload));

export const listenStatusMenuNavigate = (handler: (target: string) => void) =>
  listen<string>("status-menu-navigate", (event) => handler(event.payload));

export const listenDesktopUpdateProgress = (
  handler: (progress: DesktopUpdateProgress) => void,
) => listen<DesktopUpdateProgress>("desktop-update-progress", (event) => handler(event.payload));

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

export const listPublicProviderModels = (providerIds: string[]) =>
  invoke<PublicProviderModelsView>("list_public_provider_models", { providerIds });

export const suggestModelPrice = (providerId: string | null, modelId: string) =>
  invoke<ModelPriceSuggestionView | null>("suggest_model_price", {
    providerId,
    modelId,
  });

export const importModelPricesForProvider = (
  upstreamName: string,
  modelIds: string[],
) => invoke<ModelPriceImportResultView>("import_model_prices_for_provider", {
  upstreamName,
  modelIds,
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

/** Refresh runtime ownership/connection overlays without running installation discovery again. */
export const getCachedAgentViews = () => invoke<AgentView[]>("get_cached_agent_views");

export const planAgentConnection = (
  agentId: AgentId,
  installationPath: string,
  options?: { expectedVersion: string },
) => invoke<ConfigPlanView>("plan_agent_connection", {
  agentId,
  installationPath,
  ...(options ? options : {}),
});

export interface CursorProviderStatusView {
  state: "disconnected" | "connected" | "repair_required";
  message: string | null;
}

export const getCursorProviderStatus = () =>
  invoke<CursorProviderStatusView>("get_cursor_provider_status");

export const configureCursorProvider = () =>
  invoke<CursorProviderStatusView>("configure_cursor_provider");

export const restoreCursorProvider = () =>
  invoke<CursorProviderStatusView>("restore_cursor_provider");

export const applyAgentPlan = (
  operationId: string,
  confirmationToken: string,
) => invoke<AgentOperationView>("apply_agent_plan", {
  operationId,
  confirmationToken,
});

export const planAgentDisconnect = (agentId: AgentId, installationPath: string) =>
  invoke<ConfigPlanView>("plan_agent_disconnect", { agentId, installationPath });

/**
 * Restore official configuration and disconnect by removing TS-managed fields
 * according to ownership records, returning the Agent to its official defaults,
 * then clearing ownership. This deterministic path does not depend on encrypted
 * snapshots or a master key. It replaces exact snapshot restoration and the
 * separate force-disconnect fallback.
 */
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
// The read-only data plane prefers local HTTP `/admin/*`, allowing the same
// frontend to run outside Tauri for direct browser development and a future
// remote console. If the proxy is stopped or a request fails, the Tauri shell
// falls back to IPC so usage and routing pages can still read drafts and the
// local database. Privileged operations such as Agent transactions, config
// writes, and secrets always use IPC and never HTTP.

const IN_TAURI = "__TAURI_INTERNALS__" in window;

let adminBase: string | null = null;
let adminKey: string | null = null;

/** Synchronize the data-plane endpoint whenever App.tsx refreshes state. */
export function setAdminEndpoint(serve: ServeView) {
  const reachable = serve.app_runtime === "running" && serve.listener_reachable;
  adminBase = reachable ? `http://${serve.listen}` : null;
  adminKey = reachable ? serve.virtual_key : null;
}

export function browserAdminEndpoint(storage: Pick<Storage, "getItem">) {
  return {
    base: `http://${storage.getItem("ts_listen") ?? "127.0.0.1:8787"}`,
    key: null,
  } as const;
}

// Browser-only mode may read only the non-sensitive listen endpoint from localStorage.
// Never persist the virtual key in Web Storage; use the Tauri shell when auth is enabled.
if (!IN_TAURI) {
  const endpoint = browserAdminEndpoint(localStorage);
  adminBase = endpoint.base;
  adminKey = endpoint.key;
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
    "无法连接本地代理：请确认 token-station serve 已启动；启用鉴权时请使用桌面 App",
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

// Preserve the semantic difference: HTTP returns the running routing table,
// while IPC fallback returns the editable draft. When the proxy runs, runtime
// state is authoritative and is what the data plane should report.
export const getRouterTable = () =>
  dataGet<RouterTableView>("/admin/router-table", () =>
    invoke<RouterTableView>("get_router_table"),
  );

export const getPlugins = () =>
  IN_TAURI
    ? invoke<PluginsView>("get_plugins")
    : dataGet<PluginsView>("/admin/plugins", () => invoke<PluginsView>("get_plugins"));

export const getEgress = () =>
  dataGet<EgressView>("/admin/egress", () => invoke<EgressView>("get_egress"));

export const checkDesktopUpdate = () =>
  invoke<DesktopUpdateView>("check_desktop_update");

export const installDesktopUpdateAndRestart = (expectedVersion: string) =>
  invoke<boolean>("install_desktop_update_and_restart", { expectedVersion });
