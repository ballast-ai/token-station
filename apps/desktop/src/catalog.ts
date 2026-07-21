// Mainstream provider presets have public fixed base URLs. Include them and require only the API key.
// Models are a recommended list that supports selection and customization. Models change, so users can add or remove them.

export interface ProviderPreset {
  id: string; // Provider identifier and default upstream name.
  label: string; // Display name
  baseUrl: string;
  models: string[]; // Recommended models
  needsKey: boolean;
  note?: string;
}

export const PROVIDER_CATALOG: ProviderPreset[] = [
  {
    id: "openai",
    label: "OpenAI",
    baseUrl: "https://api.openai.com/v1",
    models: [
      "gpt-5.6-sol",
      "gpt-5.6-terra",
      "gpt-5.6-luna",
      "gpt-5.5",
      "gpt-5.5-mini",
      "gpt-4.1",
      "o4-mini",
    ],
    needsKey: true,
  },
  {
    id: "deepseek",
    label: "DeepSeek",
    baseUrl: "https://api.deepseek.com/v1",
    models: ["deepseek-v4-flash", "deepseek-v4-pro"],
    needsKey: true,
  },
  {
    id: "glm_cn",
    label: "智谱 GLM（中国）",
    baseUrl: "https://open.bigmodel.cn/api/paas/v4",
    models: ["glm-5.2", "glm-5.1", "glm-5"],
    needsKey: true,
    note: "中国开放平台通用按量 API；不要使用 Z.AI 或 Coding Plan 的 Key。",
  },
  {
    id: "glm",
    label: "智谱 GLM（国际）",
    baseUrl: "https://api.z.ai/api/paas/v4",
    models: ["glm-5.2", "glm-5.1", "glm-4.6"],
    needsKey: true,
    note: "Z.AI 国际站通用按量 API。",
  },
  {
    id: "glm_coding",
    label: "智谱 GLM（Coding Plan）",
    baseUrl: "https://api.z.ai/api/coding/paas/v4",
    models: ["glm-5.2", "glm-4.6"],
    needsKey: true,
    note: "仅限 Z.AI Coding Plan 的 Key，不能替代通用按量 API。",
  },
  {
    id: "kimi",
    label: "Moonshot Kimi（中国）",
    baseUrl: "https://api.moonshot.cn/v1",
    models: ["kimi-k3", "kimi-k2.6", "moonshot-v1-128k", "moonshot-v1-32k"],
    needsKey: true,
  },
  {
    id: "kimi_global",
    label: "Moonshot Kimi（国际）",
    baseUrl: "https://api.moonshot.ai/v1",
    models: ["kimi-k3", "kimi-k2.6"],
    needsKey: true,
    note: "仅适用于 Moonshot 国际开放平台账号。",
  },
  {
    id: "qwen",
    label: "阿里云百炼 Qwen（中国）",
    baseUrl: "https://dashscope.aliyuncs.com/compatible-mode/v1",
    models: ["qwen3.7-max", "qwen3.7-plus", "qwen3.6-flash", "qwen3-coder-plus"],
    needsKey: true,
  },
  {
    id: "qwen_us",
    label: "阿里云百炼 Qwen（美国）",
    baseUrl: "https://dashscope-us.aliyuncs.com/compatible-mode/v1",
    models: ["qwen-plus"],
    needsKey: true,
    note: "仅适用于美国（弗吉尼亚）地域的 API Key；其他国际地域需要 Workspace ID，请使用自定义供应商。",
  },
  {
    id: "minimax_cn",
    label: "MiniMax（中国）",
    baseUrl: "https://api.minimaxi.com/v1",
    models: ["MiniMax-M3", "MiniMax-M2.7", "MiniMax-M2.5"],
    needsKey: true,
    note: "中国开放平台；与国际站 Key 不通用。",
  },
  {
    id: "minimax_global",
    label: "MiniMax（国际）",
    baseUrl: "https://api.minimax.io/v1",
    models: ["MiniMax-M3", "MiniMax-M2.7", "MiniMax-M2.5"],
    needsKey: true,
    note: "国际开放平台；与中国站 Key 不通用。",
  },
  {
    id: "groq",
    label: "Groq",
    baseUrl: "https://api.groq.com/openai/v1",
    models: [
      "openai/gpt-oss-120b",
      "openai/gpt-oss-20b",
      "llama-3.3-70b-versatile",
      "llama-3.1-8b-instant",
      "moonshotai/kimi-k2-instruct",
    ],
    needsKey: true,
  },
  {
    id: "nvidia_nim",
    label: "NVIDIA NIM",
    baseUrl: "https://integrate.api.nvidia.com/v1",
    models: ["openai/gpt-oss-120b", "openai/gpt-oss-20b", "meta/llama-3.3-70b-instruct"],
    needsKey: true,
    note: "NVIDIA 托管 API Catalog；自托管 NIM 请使用自定义 Base URL。",
  },
  {
    id: "mistral",
    label: "Mistral AI",
    baseUrl: "https://api.mistral.ai/v1",
    models: ["mistral-medium-3-5", "mistral-small-2603"],
    needsKey: true,
  },
  {
    id: "xai",
    label: "xAI Grok",
    baseUrl: "https://api.x.ai/v1",
    models: ["grok-4.5", "grok-4.3", "grok-4"],
    needsKey: true,
    note: "xAI 已将 Chat Completions 标记为旧接口；本预设仅用于当前 OpenAI-compatible 兼容链路。",
  },
  {
    id: "volcengine_ark",
    label: "火山方舟（中国标准 API）",
    baseUrl: "https://ark.cn-beijing.volces.com/api/v3",
    models: ["doubao-seed-2-1-pro-260628"],
    needsKey: true,
    note: "中国区标准按量 API；与 Coding Plan 的 Key 和网关不同。",
  },
  {
    id: "volcengine_ark_coding",
    label: "火山方舟（Coding Plan）",
    baseUrl: "https://ark.cn-beijing.volces.com/api/coding/v3",
    models: ["ark-code-latest"],
    needsKey: true,
    note: "仅限火山方舟 Coding Plan。",
  },
  {
    id: "byteplus_ark",
    label: "BytePlus ModelArk（标准 API）",
    baseUrl: "https://ark.ap-southeast.bytepluses.com/api/v3",
    models: ["seed-2-0-lite-260228", "seed-1-8-251228", "seed-1-6-250915"],
    needsKey: true,
    note: "BytePlus 国际站标准按量 API。",
  },
  {
    id: "byteplus_ark_coding",
    label: "BytePlus ModelArk（Coding Plan）",
    baseUrl: "https://ark.ap-southeast.bytepluses.com/api/coding/v3",
    models: ["ark-code-latest"],
    needsKey: true,
    note: "仅限 BytePlus Coding Plan。",
  },
  {
    id: "siliconflow",
    label: "硅基流动 SiliconFlow",
    baseUrl: "https://api.siliconflow.cn/v1",
    models: ["deepseek-ai/DeepSeek-V3", "Qwen/Qwen2.5-72B-Instruct"],
    needsKey: true,
  },
  {
    id: "openrouter",
    label: "OpenRouter",
    baseUrl: "https://openrouter.ai/api/v1",
    models: ["anthropic/claude-sonnet-4", "openai/gpt-5.5", "deepseek/deepseek-chat"],
    needsKey: true,
  },
  {
    id: "ollama",
    label: "本地 Ollama",
    baseUrl: "http://127.0.0.1:11434/v1",
    models: ["llama3.3", "qwen2.5", "deepseek-r1"],
    needsKey: false,
  },
];

export const CUSTOM_ID = "__custom__";
