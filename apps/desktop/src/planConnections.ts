import type { ProviderView } from "./api";
import type { LocalizedCopy } from "./components/LanguageProvider";

export type PlanConnectionKind = "codex" | "chatgpt";

export function planConnectionTitle(kind: PlanConnectionKind, copy: LocalizedCopy): string {
  return kind === "codex"
    ? copy("Local Codex login", "本机 Codex 登录", "本機 Codex 登入", "ローカル Codex サインイン")
    : copy("ChatGPT plan", "ChatGPT 套餐", "ChatGPT 方案", "ChatGPT プラン");
}

export function providerPlanKind(provider: ProviderView): PlanConnectionKind | null {
  if (provider.provider === "codex-plan") return "codex";
  if (provider.provider === "chatgpt-plan") return "chatgpt";
  return null;
}
