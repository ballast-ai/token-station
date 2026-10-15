import { describe, expect, it } from "vitest";
import { providerDisplayName, upstreamDisplayName } from "./providerPresentation";

const channels = [
  ["codex-plan", "Codex-plan"],
  ["chatgpt-plan", "ChatGPT-plan"],
  ["claude-plan", "Claude-plan"],
  ["copilot-plan", "Copilot-plan"],
  ["gemini-cli-plan", "Gemini CLI-plan"],
  ["antigravity-plan", "Antigravity-plan"],
] as const;

describe("subscription Provider presentation", () => {
  it.each(channels)("shows a short name for %s without exposing its account ID", (provider, label) => {
    const entry = { name: "account_" + "a".repeat(64), provider };
    expect(providerDisplayName(entry)).toBe(label);
    expect(upstreamDisplayName(entry.name, [entry])).toBe(label);
    expect(entry.name).toBe("account_" + "a".repeat(64));
  });

  it("distinguishes multiple accounts with stable short ordinals", () => {
    const entries = [
      { name: "second-account", provider: "codex-plan" },
      { name: "first-account", provider: "codex-plan" },
    ];
    expect(entries.map(entry => providerDisplayName(entry, entries))).toEqual(["Codex-plan 2", "Codex-plan 1"]);
    expect(upstreamDisplayName("first-account", [...entries].reverse())).toBe("Codex-plan 1");
  });

  it("preserves ordinary names and unknown references without guessing from prefixes", () => {
    const entry = { name: "codex_custom_account", provider: "openai" };
    expect(providerDisplayName(entry)).toBe(entry.name);
    expect(upstreamDisplayName("antigravity_plan_deleted", [entry])).toBe("antigravity_plan_deleted");
    expect(providerDisplayName({ name: "tokenstation_2", provider: "openai", managed_route: true })).toBe("Token-station 2");
  });
});
