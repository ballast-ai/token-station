import { describe, expect, it } from "vitest";
import { PROVIDER_CATALOG } from "./catalog";
import {
  applyPublicProviderModels,
  listModelOfferings,
  searchModelOfferings,
} from "./modelCatalog";

describe("model offering catalog", () => {
  it("projects every provider suggestion into one resolvable offering", () => {
    const offerings = listModelOfferings();
    const expectedCount = PROVIDER_CATALOG.reduce(
      (total, provider) => total + provider.models.length,
      0,
    );

    expect(offerings).toHaveLength(expectedCount);
    expect(new Set(offerings.map((offering) => offering.id)).size).toBe(offerings.length);

    for (const offering of offerings) {
      const provider = PROVIDER_CATALOG.find((entry) => entry.id === offering.provider.id);
      expect(provider?.models).toContain(offering.upstreamModelId);
      expect(offering.sourceUrl).toMatch(/^https:\/\//);
      expect(offering.verifiedAt).toMatch(/^\d{4}-\d{2}-\d{2}$/);
    }
  });

  it("maps GLM-5.2 aliases to verified official and managed channels", () => {
    const offerings = searchModelOfferings("glm-5.2")
      .filter((offering) => offering.model.id === "zhipu/glm-5.2");
    const providerIds = offerings.map((offering) => offering.provider.id);

    expect(providerIds).toEqual(expect.arrayContaining([
      "glm_cn",
      "glm",
      "glm_coding",
      "wecoding",
      "qwen",
      "siliconflow",
      "siliconflow_global",
    ]));
    expect(offerings.every((offering) => offering.model.label === "glm-5.2")).toBe(true);
    expect(offerings.find((offering) => offering.provider.id === "qwen"))
      .toMatchObject({ upstreamModelId: "ZHIPU/GLM-5.2", deliveryClass: "managed" });
    expect(offerings.find((offering) => offering.provider.id === "siliconflow"))
      .toMatchObject({ upstreamModelId: "zai-org/GLM-5.2", deliveryClass: "managed" });
    expect(offerings.find((offering) => offering.provider.id === "wecoding"))
      .toMatchObject({ upstreamModelId: "glm-5.2", deliveryClass: "managed" });
    expect(offerings.find((offering) => offering.provider.id === "glm_cn"))
      .toMatchObject({ upstreamModelId: "glm-5.2", deliveryClass: "official" });
  });

  it("searches the canonical model through any known upstream alias", () => {
    const alibabaAlias = searchModelOfferings("ZHIPU/GLM-5.2")
      .filter((offering) => offering.model.id === "zhipu/glm-5.2");
    const siliconFlowAlias = searchModelOfferings("zai-org GLM 5.2")
      .filter((offering) => offering.model.id === "zhipu/glm-5.2");

    expect(alibabaAlias.map((offering) => offering.provider.id)).toContain("glm_cn");
    expect(siliconFlowAlias.map((offering) => offering.provider.id)).toContain("qwen");
  });

  it("searches localized provider channel names", () => {
    const offerings = searchModelOfferings("Alibaba GLM")
      .filter((offering) => offering.model.id === "zhipu/glm-5.2");

    expect(offerings.map((offering) => offering.provider.id)).toEqual(["qwen"]);
  });

  it("ranks newer numeric model versions before older versions", () => {
    const provider = PROVIDER_CATALOG.find((entry) => entry.id === "glm_cn");
    expect(provider).toBeDefined();

    const offerings = searchModelOfferings("glm", [{
      ...provider!,
      models: [
        "glm-4.7",
        "glm-5",
        "glm-5.2-highspeed",
        "glm-5.2",
        "glm-5.3",
        "autoglm-phone-9b",
      ],
    }]);

    expect(offerings.map((offering) => offering.upstreamModelId)).toEqual([
      "glm-5.3",
      "glm-5.2-highspeed",
      "glm-5.2",
      "glm-5",
      "glm-4.7",
      "autoglm-phone-9b",
    ]);
  });

  it("replaces covered suggestions, preserves explicit offerings, and keeps uncovered providers", () => {
    const effective = applyPublicProviderModels(PROVIDER_CATALOG, {
      providers: {
        openai: ["gpt-current"],
        qwen: ["glm-5.2", "qwen-current"],
        deepseek: [],
        gemini: ["gemini-safe", "gemini-evil\u202e"],
      },
      source: "live",
      fetched_at_ms: 42,
      unavailable_provider_ids: ["volcengine_ark"],
    });

    expect(effective.find((provider) => provider.id === "openai")?.models)
      .toEqual(["gpt-current"]);
    expect(effective.find((provider) => provider.id === "qwen")?.models)
      .toEqual(["glm-5.2", "qwen-current", "ZHIPU/GLM-5.2"]);
    expect(effective.find((provider) => provider.id === "volcengine_ark")?.models)
      .toEqual(PROVIDER_CATALOG.find((provider) => provider.id === "volcengine_ark")?.models);
    expect(effective.find((provider) => provider.id === "deepseek")?.models)
      .toEqual(PROVIDER_CATALOG.find((provider) => provider.id === "deepseek")?.models);
    expect(effective.find((provider) => provider.id === "gemini")?.models)
      .toEqual(PROVIDER_CATALOG.find((provider) => provider.id === "gemini")?.models);
  });
});
