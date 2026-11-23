import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";

/** Non-secret account views. OAuth credentials remain in the Host. */
export interface ChatGPTAccountView {
  id: string;
  email: string | null;
  label: string;
  state: "ready" | "identity_only" | "reauthentication_required" | "signed_out";
  expires_at: number | null;
}
export interface ChatGPTPlanModel {
  id: string;
  name: string;
  context_window: number | null;
  supports_images: boolean | null;
}
export interface ChatGPTLoginAttempt { id: string; authorization_url: string }
export interface ChatGPTLoginStatus {
  id: string;
  state: "pending" | "complete" | "cancelled" | "failed";
  account_id: string | null;
  error: string | null;
}
export interface ChatGPTSignOutResult { remote_revoked: boolean; warning: string | null }

export const listChatGPTAccounts = () => invoke<ChatGPTAccountView[]>("chatgpt_accounts_list");
export const startChatGPTLogin = (accountId?: string) =>
  invoke<ChatGPTLoginAttempt>("chatgpt_login_start", { accountId: accountId ?? null });
export const getChatGPTLoginStatus = (attemptId: string) =>
  invoke<ChatGPTLoginStatus>("chatgpt_login_status", { attemptId });
export const cancelChatGPTLogin = (attemptId: string) =>
  invoke<void>("chatgpt_login_cancel", { attemptId });
export const getChatGPTModels = (accountId: string) =>
  invoke<ChatGPTPlanModel[]>("chatgpt_models", { accountId });
export const signOutChatGPT = (accountId: string) =>
  invoke<ChatGPTSignOutResult>("chatgpt_sign_out", { accountId });
export const connectChatGPTProvider = (accountId: string) =>
  invoke<string>("chatgpt_connect_provider", { accountId });

export async function openChatGPTAuthorization(authorizationUrl: string): Promise<void> {
  const url = new URL(authorizationUrl);
  if (url.origin !== "https://auth.openai.com" || url.username || url.password
    || url.pathname !== "/api/accounts/authorize" || url.hash) {
    throw new Error("Invalid ChatGPT authorization address.");
  }
  await openUrl(url.href);
}
export const openChatGPTUsage = () => openUrl("https://chatgpt.com/settings/usage");
