import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  AGENT_VISIBILITY_STORAGE_KEY,
  readHiddenAgentIds,
  readShownUndetectedAgentIds,
  SHOWN_UNDETECTED_AGENT_IDS_STORAGE_KEY,
  updateHiddenAgentIds,
  writeHiddenAgentIds,
  writeShownUndetectedAgentIds,
} from "./AgentVisibilityPreferences";

beforeEach(() => {
  window.localStorage.clear();
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("AgentVisibilityPreferences", () => {
  it("uses an empty hidden set when no preference exists", () => {
    expect(readHiddenAgentIds()).toEqual(new Set());
  });

  it("filters malformed entries, deduplicates IDs, and returns stable order", () => {
    window.localStorage.setItem(
      AGENT_VISIBILITY_STORAGE_KEY,
      JSON.stringify([
        "openclaw",
        "",
        null,
        "Claude-Code",
        "open_code",
        "-codex",
        "open--code",
        "a".repeat(65),
        "codex",
        "openclaw",
        "claude-code",
      ]),
    );

    expect([...readHiddenAgentIds()]).toEqual([
      "claude-code",
      "codex",
      "openclaw",
    ]);
  });

  it.each([
    ["malformed JSON", "{"],
    ["an object", JSON.stringify({ hidden: ["codex"] })],
    ["a string", JSON.stringify("codex")],
    ["null", "null"],
  ])("falls back to an empty set for %s", (_label, raw) => {
    window.localStorage.setItem(AGENT_VISIBILITY_STORAGE_KEY, raw);
    expect(readHiddenAgentIds()).toEqual(new Set());
  });

  it("keeps at most 64 valid IDs when reading", () => {
    const ids = Array.from(
      { length: 70 },
      (_, index) => `agent-${String(index).padStart(2, "0")}`,
    ).reverse();
    window.localStorage.setItem(
      AGENT_VISIBILITY_STORAGE_KEY,
      JSON.stringify(ids),
    );

    const hidden = [...readHiddenAgentIds()];
    expect(hidden).toHaveLength(64);
    expect(hidden[0]).toBe("agent-00");
    expect(hidden[63]).toBe("agent-63");
  });

  it("preserves a valid hidden ID that is not in the current Registry", () => {
    window.localStorage.setItem(
      AGENT_VISIBILITY_STORAGE_KEY,
      JSON.stringify(["retired-agent"]),
    );

    expect(readHiddenAgentIds()).toEqual(new Set(["retired-agent"]));
  });

  it("persists an explicit request to show an undetected Agent separately", () => {
    expect(writeShownUndetectedAgentIds(new Set(["gemini-cli"]))).toBe(true);

    expect(readShownUndetectedAgentIds()).toEqual(new Set(["gemini-cli"]));
    expect(window.localStorage.getItem(SHOWN_UNDETECTED_AGENT_IDS_STORAGE_KEY)).toBe(
      JSON.stringify(["gemini-cli"]),
    );
    expect(window.localStorage.getItem(AGENT_VISIBILITY_STORAGE_KEY)).toBeNull();
  });

  it("returns an empty set when localStorage cannot be read", () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementationOnce(() => {
      throw new Error("storage denied");
    });

    expect(readHiddenAgentIds()).toEqual(new Set());
  });

  it("persists valid IDs in stable sorted order", () => {
    writeHiddenAgentIds(new Set([
      "openclaw",
      "invalid_id",
      "",
      "codex",
      "claude-code",
      "open--code",
    ]));

    expect(window.localStorage.getItem(AGENT_VISIBILITY_STORAGE_KEY)).toBe(
      JSON.stringify(["claude-code", "codex", "openclaw"]),
    );
  });

  it("persists at most 64 IDs using a stable lexical subset", () => {
    const ids = new Set(Array.from(
      { length: 70 },
      (_, index) => `agent-${String(index).padStart(2, "0")}`,
    ).reverse());

    writeHiddenAgentIds(ids);

    const stored = JSON.parse(
      window.localStorage.getItem(AGENT_VISIBILITY_STORAGE_KEY) ?? "[]",
    ) as string[];
    expect(stored).toHaveLength(64);
    expect(stored[0]).toBe("agent-00");
    expect(stored[63]).toBe("agent-63");
  });

  it("keeps the latest explicit hide when the bounded set is already full", () => {
    const full = new Set(Array.from(
      { length: 64 },
      (_, index) => `stale-${String(index).padStart(2, "0")}`,
    ));

    const next = updateHiddenAgentIds(full, "zz-current-agent", true);
    writeHiddenAgentIds(next);

    expect(next.size).toBe(64);
    expect(next.has("zz-current-agent")).toBe(true);
    expect(next.has("stale-63")).toBe(false);
    expect(window.localStorage.getItem(AGENT_VISIBILITY_STORAGE_KEY)).toBe(
      JSON.stringify([...next]),
    );
  });

  it("removes a hidden ID through the same normalized update path", () => {
    expect(updateHiddenAgentIds(
      new Set(["openclaw", "codex"]),
      "codex",
      false,
    )).toEqual(new Set(["openclaw"]));
  });

  it("does not throw when localStorage cannot be written", () => {
    vi.spyOn(Storage.prototype, "setItem").mockImplementationOnce(() => {
      throw new Error("storage denied");
    });

    expect(writeHiddenAgentIds(new Set(["codex"]))).toBe(false);
  });
});
