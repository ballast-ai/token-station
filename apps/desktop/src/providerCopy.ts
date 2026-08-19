const ENGLISH_PROVIDER_NAMES: Record<string, string> = {
  glm_cn: "Zhipu GLM (China)",
  glm: "Zhipu GLM (Global)",
  glm_coding: "Zhipu GLM (Coding Plan)",
  kimi: "Moonshot Kimi (China)",
  kimi_global: "Moonshot Kimi (Global)",
  qwen: "Alibaba Cloud Model Studio (China)",
  qwen_singapore: "Alibaba Cloud Model Studio (Singapore)",
  qwen_us: "Alibaba Cloud Model Studio (US)",
  minimax_cn: "MiniMax (China)",
  minimax_global: "MiniMax (Global)",
  volcengine_ark: "Volcengine Ark (China Standard API)",
  volcengine_ark_coding: "Volcengine Ark (Coding Plan)",
  byteplus_ark: "BytePlus ModelArk (Standard API)",
  byteplus_ark_coding: "BytePlus ModelArk (Coding Plan)",
  siliconflow: "SiliconFlow (China)",
  siliconflow_global: "SiliconFlow (Global)",
  qianfan: "Baidu Qianfan ModelBuilder",
  hunyuan: "Tencent Hunyuan",
  stepfun: "StepFun (Standard API)",
  stepfun_plan: "StepFun (Step Plan)",
  ollama: "Local Ollama",
};

export function englishProviderName(id: string, fallback: string): string {
  return ENGLISH_PROVIDER_NAMES[id]
    ?? fallback
      .replace(/（中国）/g, " (China)")
      .replace(/（国际）/g, " (Global)")
      .replace(/（全球）/g, " (Global)")
      .replace(/（新加坡）/g, " (Singapore)")
      .replace(/（美国）/g, " (US)")
      .replace(/（标准 API）/g, " (Standard API)")
      .replace(/（Coding Plan）/g, " (Coding Plan)");
}
