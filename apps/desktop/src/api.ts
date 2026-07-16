import { invoke } from "@tauri-apps/api/core";

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

export interface ServeView {
  running: boolean;
  listen: string;
  virtual_key: string | null;
}

export type TierSlot = "high" | "mid" | "low";

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
  serve: ServeView;
  config_error: string | null;
  settings: SettingsView;
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

export const setTier = (
  slot: TierSlot,
  upstream: string | null,
  model: string | null,
) => invoke<StateView>("set_tier", { slot, upstream, model });

export const saveConfig = () => invoke<StateView>("save_config");

export const serveStart = () => invoke<StateView>("serve_start");
export const serveStop = () => invoke<StateView>("serve_stop");

export type AgentKind = "cc" | "codex" | "opencode";

export const connectAgent = (kind: AgentKind) =>
  invoke<string>("connect_agent", { kind });

export const setSettings = (auth: boolean, metrics: boolean) =>
  invoke<StateView>("set_settings", { auth, metrics });

export const getStats = (since: string, by: string | null) =>
  invoke<StatsView>("get_stats", { since, by });

export const getRouterTable = () =>
  invoke<RouterTableView>("get_router_table");

export const getPlugins = () => invoke<PluginsView>("get_plugins");

export const checkUpgrade = () => invoke<UpgradeView>("check_upgrade");
