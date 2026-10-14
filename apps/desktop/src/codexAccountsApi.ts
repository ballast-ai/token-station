import { invoke } from "@tauri-apps/api/core";
import type { ChatGPTPlanModel } from "./chatgptPlanApi";

/** Safe source identity only. Codex credentials remain inside the Host. */
export interface LocalCodexAccount {
  id: string;
  label: string;
  email: string | null;
  state: "ready" | "missing" | "api_key_only" | "expired" | "unsupported_storage" | "account_changed" | "invalid_source";
  account_id: string;
  client_version: string;
}
export const listCodexAccounts = () => invoke<LocalCodexAccount[]>("codex_accounts_list");
export const discoverCodexAccount = () => invoke<LocalCodexAccount>("codex_account_discover");
export const getCodexModels = (accountId: string) => invoke<ChatGPTPlanModel[]>("codex_account_models", { accountId });
export const disconnectCodexAccount = (accountId: string) => invoke<void>("codex_account_disconnect", { accountId });
export const connectCodexProvider = (accountId?: string) => invoke<string>("codex_connect_provider", { accountId: accountId ?? null });

/** Adds and saves a provider only when a usable local login is newly discovered. */
export const autoConnectCodexProvider = () => invoke<boolean>("codex_auto_connect_provider");
