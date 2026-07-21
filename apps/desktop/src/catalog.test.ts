import { describe, expect, it } from "vitest";
import { PROVIDER_CATALOG } from "./catalog";

const byId = new Map(PROVIDER_CATALOG.map((preset) => [preset.id, preset]));

describe("provider catalog", () => {
  it("keeps identifiers and suggested models unique", () => {
    expect(byId.size).toBe(PROVIDER_CATALOG.length);
    for (const preset of PROVIDER_CATALOG) {
      expect(preset.id.length).toBeGreaterThan(0);
      expect(preset.label.length).toBeGreaterThan(0);
      expect(preset.models.length).toBeGreaterThan(0);
      expect(new Set(preset.models).size).toBe(preset.models.length);
    }
  });

  it("uses normalized URLs for remote presets", () => {
    for (const preset of PROVIDER_CATALOG.filter((entry) => entry.id !== "ollama")) {
      expect(preset.baseUrl).toMatch(/^https:\/\//);
      expect(preset.baseUrl.endsWith("/")).toBe(false);
      expect(preset.baseUrl.includes("{")).toBe(false);
    }
  });

  it("contains every required M1 family and endpoint variant", () => {
    const expected = {
      "minimax-cn": "https://api.minimaxi.com/v1",
      "minimax-global": "https://api.minimax.io/v1",
      "nvidia-nim": "https://integrate.api.nvidia.com/v1",
      mistral: "https://api.mistral.ai/v1",
      xai: "https://api.x.ai/v1",
      "volcengine-ark": "https://ark.cn-beijing.volces.com/api/v3",
      "volcengine-ark-coding": "https://ark.cn-beijing.volces.com/api/coding/v3",
      "byteplus-ark": "https://ark.ap-southeast.bytepluses.com/api/v3",
      "byteplus-ark-coding": "https://ark.ap-southeast.bytepluses.com/api/coding/v3",
    } as const;

    for (const [id, baseUrl] of Object.entries(expected)) {
      expect(byId.get(id)?.baseUrl).toBe(baseUrl);
      expect(byId.get(id)?.needsKey).toBe(true);
    }
  });

  it("does not collapse regional and plan-specific credentials", () => {
    expect(byId.get("glm")?.baseUrl).not.toBe(byId.get("glm-coding")?.baseUrl);
    expect(byId.get("kimi")?.baseUrl).not.toBe(byId.get("kimi-global")?.baseUrl);
    expect(byId.get("qwen")?.baseUrl).not.toBe(byId.get("qwen-us")?.baseUrl);
    expect(byId.get("minimax-cn")?.baseUrl).not.toBe(byId.get("minimax-global")?.baseUrl);
    expect(byId.get("volcengine-ark")?.baseUrl).not.toBe(byId.get("volcengine-ark-coding")?.baseUrl);
    expect(byId.get("byteplus-ark")?.baseUrl).not.toBe(byId.get("byteplus-ark-coding")?.baseUrl);
  });
});
