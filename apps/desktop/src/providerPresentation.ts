import type { ProviderView } from "./api";

export const ENTERPRISE_PROVIDER_ID = "tokenstation";
export const ENTERPRISE_PROVIDER_NAME = "Token-station";

type ProviderIdentity = Pick<ProviderView, "name" | "managed_route" | "provider">;

const SUBSCRIPTION_PROVIDER_NAMES: Record<string, string> = {
  "codex-plan": "Codex-plan",
  "chatgpt-plan": "ChatGPT-plan",
  "claude-plan": "Claude-plan",
  "copilot-plan": "Copilot-plan",
  "gemini-cli-plan": "Gemini CLI-plan",
  "antigravity-plan": "Antigravity-plan",
};

export function providerDisplayName(
  provider: ProviderIdentity,
  providers: readonly ProviderIdentity[] = [],
): string {
  const subscriptionName = Object.prototype.hasOwnProperty.call(SUBSCRIPTION_PROVIDER_NAMES, provider.provider)
    ? SUBSCRIPTION_PROVIDER_NAMES[provider.provider] : undefined;
  if (subscriptionName) {
    const peers = providers.filter(item => item.provider === provider.provider).map(item => item.name).sort();
    const index = peers.indexOf(provider.name);
    return peers.length > 1 && index >= 0 ? `${subscriptionName} ${index + 1}` : subscriptionName;
  }
  if (!provider.managed_route) return provider.name;
  if (provider.name === ENTERPRISE_PROVIDER_ID) return ENTERPRISE_PROVIDER_NAME;
  const managedIndex = /^tokenstation_(\d+)$/.exec(provider.name)?.[1];
  return managedIndex ? `${ENTERPRISE_PROVIDER_NAME} ${managedIndex}` : provider.name;
}

export function upstreamDisplayName(upstream: string, providers: readonly ProviderIdentity[]): string {
  const provider = providers.find(item => item.name === upstream);
  return provider ? providerDisplayName(provider, providers) : upstream;
}
