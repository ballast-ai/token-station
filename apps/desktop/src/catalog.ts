// OpenAI Chat Completions provider presets. Base URLs, example models, and
// categories follow official sources. Models are editable suggestions because
// catalogs change; they do not replace runtime /models discovery.

export type ProviderServiceClass = "first_party" | "managed_inference" | "self_hosted" | "aggregator";

export interface ProviderPreset {
  id: string; // Provider identifier and default upstream name.
  label: string;
  baseUrl: string;
  models: string[];
  needsKey: boolean;
  note?: string;
  local?: boolean; // Locally hosted provider such as Ollama or LM Studio; selected presets default to local models.
  protocol: "openai_chat_completions";
  region: string;
  subscription: string;
  serviceClass: ProviderServiceClass;
  officialDocs: string;
  modelDocs: string;
  verifiedAt: "2026-07-22";
}

type VerifiedPreset = Omit<ProviderPreset, "protocol" | "verifiedAt" | "modelDocs"> & { modelDocs?: string };

const verified = (preset: VerifiedPreset): ProviderPreset => ({
  ...preset,
  protocol: "openai_chat_completions",
  modelDocs: preset.modelDocs ?? preset.officialDocs,
  verifiedAt: "2026-07-22",
});

export const catalogGroupLabel = (preset: ProviderPreset): string =>
  `OpenAI Chat · ${preset.region} · ${preset.subscription}`;

const OFFICIAL_AND_MANAGED_PRESETS: ProviderPreset[] = [
  verified({
    id: "openai", label: "OpenAI", baseUrl: "https://api.openai.com/v1",
    models: ["gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna", "gpt-5.5", "gpt-5.5-mini", "gpt-4.1", "o4-mini"],
    needsKey: true, region: "全球", subscription: "按量 API", serviceClass: "first_party",
    officialDocs: "https://developers.openai.com/api/docs/guides/latest-model",
  }),
  verified({
    id: "anthropic", label: "Anthropic Claude", baseUrl: "https://api.anthropic.com/v1",
    models: ["claude-opus-4-8", "claude-sonnet-4-6", "claude-haiku-4-5-20251001"], needsKey: true,
    region: "全球", subscription: "按量 API", serviceClass: "first_party",
    officialDocs: "https://docs.claude.com/en/api/openai-sdk",
    note: "经 Anthropic 官方 OpenAI SDK 兼容端点接入；prompt cache 等原生能力在兼容模式下不可用。",
  }),
  verified({
    id: "gemini", label: "Google Gemini API", baseUrl: "https://generativelanguage.googleapis.com/v1beta/openai",
    models: ["gemini-3.6-flash"], needsKey: true, region: "全球", subscription: "按量 API", serviceClass: "first_party",
    officialDocs: "https://ai.google.dev/gemini-api/docs/openai",
  }),
  verified({
    id: "deepseek", label: "DeepSeek", baseUrl: "https://api.deepseek.com/v1",
    models: ["deepseek-v4-flash", "deepseek-v4-pro"], needsKey: true, region: "中国", subscription: "按量 API", serviceClass: "first_party",
    officialDocs: "https://api-docs.deepseek.com/",
  }),
  verified({
    id: "glm_cn", label: "智谱 GLM（中国）", baseUrl: "https://open.bigmodel.cn/api/paas/v4",
    models: ["glm-5.2", "glm-5.1", "glm-5"], needsKey: true, region: "中国", subscription: "按量 API", serviceClass: "first_party",
    officialDocs: "https://docs.bigmodel.cn/cn/guide/develop/openai/introduction",
    note: "中国开放平台通用按量 API；不要使用 Z.AI 或 Coding Plan 的 Key。",
  }),
  verified({
    id: "glm", label: "智谱 GLM（国际）", baseUrl: "https://api.z.ai/api/paas/v4",
    models: ["glm-5.2", "glm-5.1", "glm-4.6"], needsKey: true, region: "全球", subscription: "按量 API", serviceClass: "first_party",
    officialDocs: "https://docs.z.ai/devpack/tool/others", note: "Z.AI 国际站通用按量 API。",
  }),
  verified({
    id: "glm_coding", label: "智谱 GLM（Coding Plan）", baseUrl: "https://api.z.ai/api/coding/paas/v4",
    models: ["glm-5.2", "glm-4.6"], needsKey: true, region: "全球", subscription: "Coding Plan", serviceClass: "first_party",
    officialDocs: "https://docs.z.ai/devpack/tool/others", note: "仅限 Z.AI Coding Plan 的 Key，不能替代通用按量 API。",
  }),
  verified({
    id: "kimi", label: "Moonshot Kimi（中国）", baseUrl: "https://api.moonshot.cn/v1",
    models: ["kimi-k3", "kimi-k2.6", "moonshot-v1-128k", "moonshot-v1-32k"], needsKey: true, region: "中国", subscription: "按量 API", serviceClass: "first_party",
    officialDocs: "https://platform.moonshot.cn/docs/guide/start-using-kimi-api",
  }),
  verified({
    id: "kimi_global", label: "Moonshot Kimi（国际）", baseUrl: "https://api.moonshot.ai/v1",
    models: ["kimi-k3", "kimi-k2.6"], needsKey: true, region: "全球", subscription: "按量 API", serviceClass: "first_party",
    officialDocs: "https://platform.moonshot.ai/docs/guide/start-using-kimi-api", note: "仅适用于 Moonshot 国际开放平台账号。",
  }),
  verified({
    id: "qwen", label: "阿里云百炼（中国）", baseUrl: "https://dashscope.aliyuncs.com/compatible-mode/v1",
    models: ["qwen3.7-max", "qwen3.7-plus", "qwen3.6-flash", "qwen3-coder-plus", "ZHIPU/GLM-5.2"], needsKey: true, region: "中国", subscription: "按量 API", serviceClass: "first_party",
    officialDocs: "https://help.aliyun.com/zh/model-studio/compatibility-of-openai-with-dashscope",
  }),
  verified({
    id: "qwen_singapore", label: "阿里云百炼（新加坡）", baseUrl: "https://dashscope-intl.aliyuncs.com/compatible-mode/v1",
    models: ["qwen-plus"], needsKey: true, region: "新加坡", subscription: "按量 API", serviceClass: "first_party",
    officialDocs: "https://www.alibabacloud.com/help/en/model-studio/compatibility-of-openai-with-dashscope",
    note: "仅适用于新加坡地域的 API Key。",
  }),
  verified({
    id: "qwen_us", label: "阿里云百炼（美国）", baseUrl: "https://dashscope-us.aliyuncs.com/compatible-mode/v1",
    models: ["qwen-plus"], needsKey: true, region: "美国", subscription: "按量 API", serviceClass: "first_party",
    officialDocs: "https://www.alibabacloud.com/help/en/model-studio/compatibility-of-openai-with-dashscope",
    note: "仅适用于美国（弗吉尼亚）地域的 API Key；其他国际地域需要 Workspace ID，请使用自定义供应商。",
  }),
  verified({
    id: "minimax_cn", label: "MiniMax（中国）", baseUrl: "https://api.minimaxi.com/v1",
    models: ["MiniMax-M3", "MiniMax-M2.7", "MiniMax-M2.5"], needsKey: true, region: "中国", subscription: "按量 API", serviceClass: "first_party",
    officialDocs: "https://platform.minimaxi.com/docs/api-reference/text-openai-api", note: "中国开放平台；与国际站 Key 不通用。",
  }),
  verified({
    id: "minimax_global", label: "MiniMax（国际）", baseUrl: "https://api.minimax.io/v1",
    models: ["MiniMax-M3", "MiniMax-M2.7", "MiniMax-M2.5"], needsKey: true, region: "全球", subscription: "按量 API", serviceClass: "first_party",
    officialDocs: "https://platform.minimax.io/docs/api-reference/text-openai-api", note: "国际开放平台；与中国站 Key 不通用。",
  }),
  verified({
    id: "groq", label: "Groq", baseUrl: "https://api.groq.com/openai/v1",
    models: ["openai/gpt-oss-120b", "openai/gpt-oss-20b", "llama-3.3-70b-versatile", "llama-3.1-8b-instant", "moonshotai/kimi-k2-instruct"],
    needsKey: true, region: "全球", subscription: "按量 API", serviceClass: "managed_inference", officialDocs: "https://console.groq.com/docs/openai",
  }),
  verified({
    id: "nvidia_nim", label: "NVIDIA API Catalog", baseUrl: "https://integrate.api.nvidia.com/v1",
    models: ["openai/gpt-oss-120b", "openai/gpt-oss-20b", "meta/llama-3.3-70b-instruct"], needsKey: true, region: "全球", subscription: "托管推理", serviceClass: "managed_inference",
    officialDocs: "https://docs.api.nvidia.com/nim/docs/api-quickstart", note: "NVIDIA 托管 API Catalog；自托管 NIM 使用本地预设。",
  }),
  verified({
    id: "mistral", label: "Mistral AI", baseUrl: "https://api.mistral.ai/v1", models: ["mistral-medium-3-5", "mistral-small-2603"],
    needsKey: true, region: "全球", subscription: "按量 API", serviceClass: "first_party", officialDocs: "https://docs.mistral.ai/api/endpoint/chat",
  }),
  verified({
    id: "xai", label: "xAI Grok", baseUrl: "https://api.x.ai/v1", models: ["grok-4.5", "grok-4.3", "grok-4"],
    needsKey: true, region: "全球", subscription: "按量 API", serviceClass: "first_party", officialDocs: "https://docs.x.ai/developers/model-capabilities/legacy/chat-completions",
    note: "xAI 已将 Chat Completions 标记为旧接口；本预设仅用于当前 OpenAI-compatible 兼容链路。",
  }),
  verified({
    id: "volcengine_ark", label: "火山方舟（中国标准 API）", baseUrl: "https://ark.cn-beijing.volces.com/api/v3", models: ["doubao-seed-2-1-pro-260628"],
    needsKey: true, region: "中国", subscription: "按量 API", serviceClass: "first_party", officialDocs: "https://www.volcengine.com/docs/82379/1494384",
    note: "中国区标准按量 API；与 Coding Plan 的 Key 和网关不同。",
  }),
  verified({
    id: "volcengine_ark_coding", label: "火山方舟（Coding Plan）", baseUrl: "https://ark.cn-beijing.volces.com/api/coding/v3", models: ["ark-code-latest"],
    needsKey: true, region: "中国", subscription: "Coding Plan", serviceClass: "first_party", officialDocs: "https://www.volcengine.com/docs/82379/2164851", note: "仅限火山方舟 Coding Plan。",
  }),
  verified({
    id: "byteplus_ark", label: "BytePlus ModelArk（标准 API）", baseUrl: "https://ark.ap-southeast.bytepluses.com/api/v3", models: ["seed-2-0-lite-260228", "seed-1-8-251228", "seed-1-6-250915"],
    needsKey: true, region: "新加坡", subscription: "按量 API", serviceClass: "first_party", officialDocs: "https://docs.byteplus.com/en/docs/ModelArk/1298459", note: "BytePlus 国际站标准按量 API。",
  }),
  verified({
    id: "byteplus_ark_coding", label: "BytePlus ModelArk（Coding Plan）", baseUrl: "https://ark.ap-southeast.bytepluses.com/api/coding/v3", models: ["ark-code-latest"],
    needsKey: true, region: "新加坡", subscription: "Coding Plan", serviceClass: "first_party", officialDocs: "https://docs.byteplus.com/en/docs/ModelArk/2164861", note: "仅限 BytePlus Coding Plan。",
  }),
  verified({
    id: "siliconflow", label: "硅基流动 SiliconFlow（中国）", baseUrl: "https://api.siliconflow.cn/v1", models: ["deepseek-ai/DeepSeek-V3", "Qwen/Qwen2.5-72B-Instruct", "zai-org/GLM-5.2"],
    needsKey: true, region: "中国", subscription: "托管推理", serviceClass: "managed_inference", officialDocs: "https://docs.siliconflow.cn/cn/api-reference/chat-completions/chat-completions",
  }),
  verified({
    id: "siliconflow_global", label: "SiliconFlow（国际）", baseUrl: "https://api.siliconflow.com/v1", models: ["deepseek-ai/DeepSeek-V3", "Qwen/Qwen2.5-72B-Instruct", "zai-org/GLM-5.2"],
    needsKey: true, region: "全球", subscription: "托管推理", serviceClass: "managed_inference", officialDocs: "https://docs.siliconflow.com/en/api-reference/chat-completions/chat-completions",
  }),
  verified({
    id: "together", label: "Together AI", baseUrl: "https://api.together.ai/v1", models: ["MiniMaxAI/MiniMax-M3"],
    needsKey: true, region: "全球", subscription: "托管推理", serviceClass: "managed_inference", officialDocs: "https://docs.together.ai/docs/inference/openai-compatibility",
  }),
  verified({
    id: "fireworks", label: "Fireworks AI", baseUrl: "https://api.fireworks.ai/inference/v1", models: ["accounts/fireworks/models/llama-v3p1-8b-instruct"],
    needsKey: true, region: "全球", subscription: "托管推理", serviceClass: "managed_inference", officialDocs: "https://docs.fireworks.ai/tools-sdks/openai-compatibility",
  }),
  verified({
    id: "deepinfra", label: "DeepInfra", baseUrl: "https://api.deepinfra.com/v1/openai", models: ["deepseek-ai/DeepSeek-V3"],
    needsKey: true, region: "全球", subscription: "托管推理", serviceClass: "managed_inference", officialDocs: "https://deepinfra.com/docs/advanced/openai_api",
  }),
  verified({
    id: "cerebras", label: "Cerebras Inference", baseUrl: "https://api.cerebras.ai/v1", models: ["llama3.1-8b"],
    needsKey: true, region: "全球", subscription: "托管推理", serviceClass: "managed_inference", officialDocs: "https://inference-docs.cerebras.ai/resources/openai",
  }),
  verified({
    id: "sambanova", label: "SambaNova Cloud", baseUrl: "https://api.sambanova.ai/v1", models: ["Meta-Llama-3.3-70B-Instruct"],
    needsKey: true, region: "全球", subscription: "托管推理", serviceClass: "managed_inference", officialDocs: "https://docs.sambanova.ai/docs/en/get-started/api-keys-urls",
  }),
  verified({
    id: "cohere", label: "Cohere", baseUrl: "https://api.cohere.ai/compatibility/v1", models: ["command-r-plus"],
    needsKey: true, region: "全球", subscription: "按量 API", serviceClass: "first_party", officialDocs: "https://docs.cohere.com/docs/compatibility-api",
  }),
  verified({
    id: "github_models", label: "GitHub Models", baseUrl: "https://models.github.ai/inference", models: ["openai/gpt-4.1"],
    needsKey: true, region: "全球", subscription: "GitHub Models", serviceClass: "managed_inference", officialDocs: "https://docs.github.com/en/rest/models/inference",
    note: "需要带 models:read 权限的 GitHub token。",
  }),
  verified({
    id: "qianfan", label: "百度千帆 ModelBuilder", baseUrl: "https://qianfan.baidubce.com/v2", models: ["deepseek-v3.1-250821"],
    needsKey: true, region: "中国", subscription: "按量 API", serviceClass: "managed_inference", officialDocs: "https://cloud.baidu.com/doc/qianfan/s/rmh4stn9m",
  }),
  verified({
    id: "hunyuan", label: "腾讯混元", baseUrl: "https://api.hunyuan.cloud.tencent.com/v1", models: ["hunyuan-turbos-latest"],
    needsKey: true, region: "中国", subscription: "按量 API", serviceClass: "first_party", officialDocs: "https://cloud.tencent.com/document/product/1729/111007",
  }),
  verified({
    id: "stepfun", label: "阶跃星辰（标准 API）", baseUrl: "https://api.stepfun.com/v1", models: ["step-3.7-flash", "step-3.5-flash-2603"],
    needsKey: true, region: "中国", subscription: "按量 API", serviceClass: "first_party", officialDocs: "https://platform.stepfun.com/docs/zh/api-reference/chat/chat-completion-create",
  }),
  verified({
    id: "stepfun_plan", label: "阶跃星辰（Step Plan）", baseUrl: "https://api.stepfun.com/step_plan/v1", models: ["step-router-v1"],
    needsKey: true, region: "中国", subscription: "Step Plan", serviceClass: "first_party", officialDocs: "https://platform.stepfun.com/docs/zh/api-reference/chat/chat-completion-create", note: "仅限 Step Plan 通道与对应凭据。",
  }),
  verified({
    id: "xiaomi_mimo", label: "Xiaomi MiMo", baseUrl: "https://api.xiaomimimo.com/v1", models: ["mimo-v2.5-pro"],
    needsKey: true, region: "全球", subscription: "按量 API", serviceClass: "first_party", officialDocs: "https://platform.xiaomimimo.com/docs/en-US/api/chat/openai-api",
  }),
  verified({
    id: "perplexity", label: "Perplexity Sonar", baseUrl: "https://api.perplexity.ai",
    models: ["sonar", "sonar-pro", "sonar-reasoning-pro", "sonar-deep-research"], needsKey: true,
    region: "全球", subscription: "按量 API", serviceClass: "first_party", officialDocs: "https://docs.perplexity.ai/docs/sonar/openai-compatibility",
    note: "联网搜索增强模型;Sonar 系列 OpenAI 兼容。",
  }),
  verified({
    id: "novita", label: "Novita AI", baseUrl: "https://api.novita.ai/v3/openai",
    models: ["deepseek/deepseek-r1", "meta-llama/llama-3.1-70b-instruct"], needsKey: true,
    region: "全球", subscription: "托管推理", serviceClass: "managed_inference", officialDocs: "https://novita.ai/docs/guides/llm-api",
  }),
  verified({
    id: "hyperbolic", label: "Hyperbolic", baseUrl: "https://api.hyperbolic.xyz/v1",
    models: ["deepseek-ai/DeepSeek-V3", "meta-llama/Meta-Llama-3.1-8B-Instruct"], needsKey: true,
    region: "全球", subscription: "托管推理", serviceClass: "managed_inference", officialDocs: "https://docs.hyperbolic.xyz/docs/inference-api",
  }),
  verified({
    id: "nebius", label: "Nebius AI Studio", baseUrl: "https://api.studio.nebius.com/v1",
    models: ["meta-llama/Meta-Llama-3.3-70B-Instruct", "Qwen/Qwen3-235B-A22B"], needsKey: true,
    region: "欧洲", subscription: "托管推理", serviceClass: "managed_inference", officialDocs: "https://docs.studio.nebius.com/quickstart",
  }),
];

// Keep Ollama as the single representative for local self-hosting. Other local
// runtimes such as LM Studio, vLLM, llama.cpp, and LocalAI use the same
// OpenAI-compatible protocol and local address, so users can connect them with
// a custom configuration marked as a local model instead of separate presets.
const SELF_HOSTED_PRESETS: ProviderPreset[] = [
  verified({
    id: "ollama", label: "本地 Ollama", baseUrl: "http://127.0.0.1:11434/v1", models: ["llama3.3", "qwen2.5", "deepseek-r1"],
    needsKey: false, local: true, region: "本机", subscription: "自托管", serviceClass: "self_hosted", officialDocs: "https://docs.ollama.com/api/openai-compatibility",
  }),
];

export const AGGREGATOR_CANDIDATES: ProviderPreset[] = [
  verified({
    id: "openrouter", label: "OpenRouter（候选，未默认收录）", baseUrl: "https://openrouter.ai/api/v1",
    models: ["anthropic/claude-sonnet-4", "openai/gpt-5.5", "deepseek/deepseek-chat"], needsKey: true,
    region: "全球", subscription: "聚合路由", serviceClass: "aggregator", officialDocs: "https://openrouter.ai/docs/api/reference/overview",
    note: "商业聚合路由；按产品边界保留为候选，未经明确确认不进入默认目录。",
  }),
];

export const PROVIDER_CATALOG: ProviderPreset[] = [...OFFICIAL_AND_MANAGED_PRESETS, ...SELF_HOSTED_PRESETS];

export const CUSTOM_ID = "__custom__";
