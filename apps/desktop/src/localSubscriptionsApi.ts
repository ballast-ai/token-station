import { invoke } from "@tauri-apps/api/core";
import type { LocalSubscriptionKind } from "./subscriptions";

/** Only safe identity and model metadata cross the renderer boundary. */
export interface SubscriptionAccount {
  id: string;
  label: string;
  email: string | null;
  state: string;
}
export interface SubscriptionModel {
  id: string;
  name: string;
  context_window: number | null;
  supports_images: boolean | null;
  supports_tools: boolean | null;
}
export const discoverSubscription = (kind: LocalSubscriptionKind) => invoke<SubscriptionAccount>("subscription_discover", { kind });
export const listSubscriptions = (kind: LocalSubscriptionKind) => invoke<SubscriptionAccount[]>("subscription_accounts_list", { kind });
export const getSubscriptionModels = (kind: LocalSubscriptionKind, accountId: string) => invoke<SubscriptionModel[]>("subscription_models", { kind, accountId });
export const connectSubscription = (kind: LocalSubscriptionKind, accountId?: string) => invoke<string>("subscription_connect_provider", { kind, accountId: accountId ?? null });
export const disconnectSubscription = (kind: LocalSubscriptionKind, accountId: string) => invoke<void>("subscription_disconnect", { kind, accountId });
