import { describe, expect, it } from "vitest";
import { resolveStatusMenuNavigation } from "./statusMenuNavigation";

describe("status menu navigation boundary", () => {
  const knownAgents = new Set(["claude-code", "codex"]);

  it.each([
    ["home", "home"],
    ["add-provider", "add-provider"],
    ["logs", "logs"],
    ["settings", "settings"],
    ["agent:codex", "agent:codex"],
  ] as const)("accepts the predefined target %s", (target, expected) => {
    expect(resolveStatusMenuNavigation(target, knownAgents)).toBe(expected);
  });

  it.each([
    "agent:unknown",
    "agent:../settings",
    "providers",
    "https://example.com",
    "",
  ])("rejects an untrusted target %s", (target) => {
    expect(resolveStatusMenuNavigation(target, knownAgents)).toBeNull();
  });

  it("ignores payloads from unrelated native events", () => {
    expect(resolveStatusMenuNavigation({ phase: "running" }, knownAgents)).toBeNull();
  });
});
