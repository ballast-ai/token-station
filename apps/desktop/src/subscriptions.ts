export const SUBSCRIPTION_CHANNELS = ["claude", "chatgpt", "chatgpt-api", "copilot", "gemini-cli", "antigravity"] as const;
export type SubscriptionChannel = typeof SUBSCRIPTION_CHANNELS[number];
export type LocalSubscriptionKind = Exclude<SubscriptionChannel, "chatgpt" | "chatgpt-api">;
export const SUBSCRIPTION_LABELS: Record<SubscriptionChannel, string> = {
  claude: "Claude", chatgpt: "ChatGPT", "chatgpt-api": "ChatGPT API",
  copilot: "Copilot", "gemini-cli": "Gemini CLI", antigravity: "Antigravity",
};
export function localSubscriptionKind(provider: string): LocalSubscriptionKind | null {
  const kinds: Record<string, LocalSubscriptionKind> = {
    "claude-plan": "claude", "copilot-plan": "copilot",
    "gemini-cli-plan": "gemini-cli", "antigravity-plan": "antigravity",
  };
  return kinds[provider] ?? null;
}
export function isSubscriptionProvider(provider: string): boolean {
  return provider === "codex-plan" || provider === "chatgpt-plan" || localSubscriptionKind(provider) !== null;
}
