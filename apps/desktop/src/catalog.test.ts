import { describe, expect, it } from "vitest";
import { AGGREGATOR_CANDIDATES, PROVIDER_CATALOG } from "./catalog";

const byId = new Map(PROVIDER_CATALOG.map((preset) => [preset.id, preset]));

describe("provider catalog", () => {
  it("ships at least fifty verified non-aggregator defaults", () => {
    expect(PROVIDER_CATALOG.length).toBeGreaterThanOrEqual(50);
    expect(PROVIDER_CATALOG.every((preset) => preset.serviceClass !== "aggregator")).toBe(true);
    expect(AGGREGATOR_CANDIDATES.length).toBeGreaterThan(0);
    expect(AGGREGATOR_CANDIDATES.every((preset) => preset.serviceClass === "aggregator")).toBe(true);
  });

  it("keeps identifiers and suggested models unique", () => {
    expect(byId.size).toBe(PROVIDER_CATALOG.length);
    for (const preset of PROVIDER_CATALOG) {
      expect(preset.id).toMatch(/^[a-z][a-z0-9_]*$/);
      expect(preset.label.length).toBeGreaterThan(0);
      expect(preset.models.length).toBeGreaterThan(0);
      expect(new Set(preset.models).size).toBe(preset.models.length);
      expect(preset.protocol).toBe("openai_chat_completions");
      expect(preset.region.length).toBeGreaterThan(0);
      expect(preset.subscription.length).toBeGreaterThan(0);
      expect(preset.officialDocs).toMatch(/^https:\/\//);
      expect(preset.modelDocs).toMatch(/^https:\/\//);
      expect(preset.verifiedAt).toBe("2026-07-22");
    }
  });

  it("uses normalized URLs for remote presets", () => {
    for (const preset of PROVIDER_CATALOG.filter((entry) => entry.serviceClass !== "self_hosted")) {
      expect(preset.baseUrl).toMatch(/^https:\/\//);
      expect(preset.baseUrl.endsWith("/")).toBe(false);
      expect(preset.baseUrl.includes("{")).toBe(false);
    }
  });

  it("uses loopback-only URLs for self-hosted defaults", () => {
    for (const preset of PROVIDER_CATALOG.filter((entry) => entry.serviceClass === "self_hosted")) {
      expect(preset.baseUrl).toMatch(/^http:\/\/(127\.0\.0\.1|localhost):\d+\//);
      expect(preset.needsKey).toBe(false);
    }
  });

  it("contains every required M1 family and endpoint variant", () => {
    const expected = {
      minimax_cn: "https://api.minimaxi.com/v1",
      minimax_global: "https://api.minimax.io/v1",
      nvidia_nim: "https://integrate.api.nvidia.com/v1",
      mistral: "https://api.mistral.ai/v1",
      xai: "https://api.x.ai/v1",
      volcengine_ark: "https://ark.cn-beijing.volces.com/api/v3",
      volcengine_ark_coding: "https://ark.cn-beijing.volces.com/api/coding/v3",
      byteplus_ark: "https://ark.ap-southeast.bytepluses.com/api/v3",
      byteplus_ark_coding: "https://ark.ap-southeast.bytepluses.com/api/coding/v3",
    } as const;

    for (const [id, baseUrl] of Object.entries(expected)) {
      expect(byId.get(id)?.baseUrl).toBe(baseUrl);
      expect(byId.get(id)?.needsKey).toBe(true);
    }
  });

  it("does not collapse regional and plan-specific credentials", () => {
    expect(byId.get("glm")?.baseUrl).not.toBe(byId.get("glm_coding")?.baseUrl);
    expect(byId.get("kimi")?.baseUrl).not.toBe(byId.get("kimi_global")?.baseUrl);
    expect(byId.get("qwen")?.baseUrl).not.toBe(byId.get("qwen_us")?.baseUrl);
    expect(byId.get("minimax_cn")?.baseUrl).not.toBe(byId.get("minimax_global")?.baseUrl);
    expect(byId.get("volcengine_ark")?.baseUrl).not.toBe(byId.get("volcengine_ark_coding")?.baseUrl);
    expect(byId.get("byteplus_ark")?.baseUrl).not.toBe(byId.get("byteplus_ark_coding")?.baseUrl);
  });
});
