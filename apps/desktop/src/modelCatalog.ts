import { PROVIDER_CATALOG, type ProviderPreset } from "./catalog";
import { englishProviderName } from "./providerCopy";

export type ModelDeliveryClass =
  | "official"
  | "managed"
  | "self_hosted"
  | "aggregated"
  | "unspecified";

export interface ModelIdentity {
  id: string;
  label: string;
  developer: string | null;
  aliases: string[];
}

export interface ModelOffering {
  id: string;
  model: ModelIdentity;
  provider: ProviderPreset;
  upstreamModelId: string;
  deliveryClass: ModelDeliveryClass;
  sourceUrl: string;
  verifiedAt: string;
}

export interface PublicProviderModelsSnapshot {
  providers: Record<string, string[]>;
  source: "live" | "cache" | "stale_cache";
  fetched_at_ms: number;
  unavailable_provider_ids: string[];
}

interface OfferingMetadata {
  model: ModelIdentity;
  deliveryClass: ModelDeliveryClass;
  sourceUrl: string;
  verifiedAt: string;
}

const GLM_5_2: ModelIdentity = {
  id: "zhipu/glm-5.2",
  label: "glm-5.2",
  developer: "Zhipu AI",
  aliases: [
    "glm",
    "glm 5.2",
    "glm-5.2",
    "ZHIPU/GLM-5.2",
    "zai-org/GLM-5.2",
  ],
};

const GLM_OFFICIAL_SOURCE = "https://docs.bigmodel.cn/cn/guide/develop/openai/introduction";
const GLM_GLOBAL_SOURCE = "https://docs.z.ai/devpack/tool/others";
const ALIBABA_GLM_SOURCE = "https://help.aliyun.com/zh/model-studio/glm-zhipu";
const SILICONFLOW_CN_SOURCE = "https://www.siliconflow.cn/models";
const SILICONFLOW_GLOBAL_SOURCE = "https://www.siliconflow.com/models";

const OFFERING_METADATA: Record<string, OfferingMetadata> = {
  "glm_cn:glm-5.2": {
    model: GLM_5_2,
    deliveryClass: "official",
    sourceUrl: GLM_OFFICIAL_SOURCE,
    verifiedAt: "2026-08-19",
  },
  "glm:glm-5.2": {
    model: GLM_5_2,
    deliveryClass: "official",
    sourceUrl: GLM_GLOBAL_SOURCE,
    verifiedAt: "2026-08-19",
  },
  "glm_coding:glm-5.2": {
    model: GLM_5_2,
    deliveryClass: "official",
    sourceUrl: GLM_GLOBAL_SOURCE,
    verifiedAt: "2026-08-19",
  },
  "qwen:ZHIPU/GLM-5.2": {
    model: GLM_5_2,
    deliveryClass: "managed",
    sourceUrl: ALIBABA_GLM_SOURCE,
    verifiedAt: "2026-08-19",
  },
  "siliconflow:zai-org/GLM-5.2": {
    model: GLM_5_2,
    deliveryClass: "managed",
    sourceUrl: SILICONFLOW_CN_SOURCE,
    verifiedAt: "2026-08-19",
  },
  "siliconflow_global:zai-org/GLM-5.2": {
    model: GLM_5_2,
    deliveryClass: "managed",
    sourceUrl: SILICONFLOW_GLOBAL_SOURCE,
    verifiedAt: "2026-08-19",
  },
};

function isValidPublicModelId(modelId: string): boolean {
  return modelId.length > 0
    && new TextEncoder().encode(modelId).length <= 512
    && [...modelId].every((character) => {
      const codePoint = character.codePointAt(0) ?? 0;
      return codePoint >= 0x21 && codePoint <= 0x7e;
    });
}

export function applyPublicProviderModels(
  providers: readonly ProviderPreset[],
  snapshot: PublicProviderModelsSnapshot | null,
): ProviderPreset[] {
  if (!snapshot) return [...providers];

  return providers.map((provider) => {
    const publicModels = snapshot.providers[provider.id];
    if (!publicModels) return provider;
    if (
      publicModels.length === 0
      || publicModels.length > 512
      || publicModels.some((model) => !isValidPublicModelId(model))
    ) {
      return provider;
    }
    const explicitModels = provider.models.filter(
      (model) => OFFERING_METADATA[`${provider.id}:${model}`] !== undefined,
    );
    return {
      ...provider,
      models: [...new Set([...publicModels, ...explicitModels])],
    };
  });
}

function defaultDeliveryClass(provider: ProviderPreset): ModelDeliveryClass {
  if (provider.serviceClass === "managed_inference") return "managed";
  if (provider.serviceClass === "self_hosted") return "self_hosted";
  if (provider.serviceClass === "aggregator") return "aggregated";
  return "unspecified";
}

function defaultModelIdentity(provider: ProviderPreset, upstreamModelId: string): ModelIdentity {
  return {
    id: `${provider.id}/${upstreamModelId}`,
    label: upstreamModelId,
    developer: null,
    aliases: [upstreamModelId],
  };
}

function normalizeSearchText(value: string): string {
  return value
    .normalize("NFKC")
    .toLocaleLowerCase()
    .replace(/[^\p{L}\p{N}]+/gu, " ");
}

function matchesSearchTerms(searchableText: string, query: string): boolean {
  const terms = normalizeSearchText(query).split(/\s+/).filter(Boolean);
  if (terms.length === 0) return true;

  const normalizedText = normalizeSearchText(searchableText);
  return terms.every((term) => normalizedText.includes(term));
}

function compareNumericModelVersions(left: ModelOffering, right: ModelOffering): number {
  const leftParts = (left.model.label.match(/\d+/g) ?? []).map(Number);
  const rightParts = (right.model.label.match(/\d+/g) ?? []).map(Number);
  const partCount = Math.max(leftParts.length, rightParts.length);
  for (let index = 0; index < partCount; index += 1) {
    const difference = (rightParts[index] ?? -1) - (leftParts[index] ?? -1);
    if (difference !== 0) return difference;
  }
  return 0;
}

function exactModelTermMatches(offering: ModelOffering, query: string): number {
  const terms = normalizeSearchText(query).split(/\s+/).filter(Boolean);
  const modelTerms = new Set(normalizeSearchText([
    offering.model.label,
    ...offering.model.aliases,
    offering.upstreamModelId,
  ].join(" ")).split(/\s+/).filter(Boolean));
  return terms.filter((term) => modelTerms.has(term)).length;
}

export function listModelOfferings(
  providers: readonly ProviderPreset[] = PROVIDER_CATALOG,
): ModelOffering[] {
  return providers.flatMap((provider) => provider.models.map((upstreamModelId) => {
    const id = `${provider.id}:${upstreamModelId}`;
    const metadata = OFFERING_METADATA[id];
    return {
      id,
      model: metadata?.model ?? defaultModelIdentity(provider, upstreamModelId),
      provider,
      upstreamModelId,
      deliveryClass: metadata?.deliveryClass ?? defaultDeliveryClass(provider),
      sourceUrl: metadata?.sourceUrl ?? provider.modelDocs,
      verifiedAt: metadata?.verifiedAt ?? provider.verifiedAt,
    };
  }));
}

export function searchModelOfferings(
  query: string,
  providers: readonly ProviderPreset[] = PROVIDER_CATALOG,
): ModelOffering[] {
  return listModelOfferings(providers)
    .filter((offering) => matchesSearchTerms([
      offering.model.id,
      offering.model.label,
      offering.model.developer ?? "",
      ...offering.model.aliases,
      offering.upstreamModelId,
      offering.provider.id,
      offering.provider.label,
      englishProviderName(offering.provider.id, offering.provider.label),
      offering.provider.region,
      offering.provider.subscription,
    ].join(" "), query))
    .sort((left, right) => (
      exactModelTermMatches(right, query) - exactModelTermMatches(left, query)
      || compareNumericModelVersions(left, right)
    ));
}
