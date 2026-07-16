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
    id: "deepseek",
    label: "DeepSeek",
    baseUrl: "https://api.deepseek.com/v1",
    models: ["deepseek-chat", "deepseek-reasoner"],
    needsKey: true,
  },
  {
    id: "openai",
    label: "OpenAI",
    baseUrl: "https://api.openai.com/v1",
    models: ["gpt-5.5", "gpt-5.5-mini", "gpt-4.1", "o4-mini"],
    needsKey: true,
  },
  {
    id: "glm",
    label: "智谱 GLM",
    baseUrl: "https://api.z.ai/api/paas/v4",
    models: ["glm-4.6", "glm-4.5", "glm-4.5-air"],
    needsKey: true,
  },
  {
    id: "glm-coding",
    label: "智谱 GLM(Coding Plan)",
    baseUrl: "https://api.z.ai/api/coding/paas/v4",
    models: ["glm-5.2", "glm-4.6"],
    needsKey: true,
    note: "仅限 Coding Plan 的 key",
  },
  {
    id: "kimi",
    label: "Moonshot Kimi",
    baseUrl: "https://api.moonshot.cn/v1",
    models: ["kimi-k2-0711-preview", "moonshot-v1-128k", "moonshot-v1-32k", "moonshot-v1-8k"],
    needsKey: true,
  },
  {
    id: "qwen",
    label: "阿里 Qwen(百炼)",
    baseUrl: "https://dashscope.aliyuncs.com/compatible-mode/v1",
    models: ["qwen-max", "qwen-plus", "qwen-turbo", "qwen3-coder-plus"],
    needsKey: true,
  },
  {
    id: "groq",
    label: "Groq",
    baseUrl: "https://api.groq.com/openai/v1",
    models: ["llama-3.3-70b-versatile", "llama-3.1-8b-instant", "moonshotai/kimi-k2-instruct"],
    needsKey: true,
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
