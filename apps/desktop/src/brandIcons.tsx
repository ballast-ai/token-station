// Brand logo map: map provider preset IDs and Agent IDs to colored @lobehub/icons
// Avatar icon. Return null for brands without an official logo, such as Hermes. The caller uses a colored initial block.
import { useState, type ComponentType } from "react";
import {
  AlibabaCloud,
  Anthropic,
  Baidu,
  Cerebras,
  Claude,
  ClaudeCode,
  Codex,
  Cohere,
  DeepInfra,
  DeepSeek,
  Fireworks,
  Gemini,
  Github,
  Grok,
  Groq,
  Hunyuan,
  HuggingFace,
  Hyperbolic,
  Kimi,
  Minimax,
  Mistral,
  ModelScope,
  Nebius,
  Novita,
  Nvidia,
  Ollama,
  OpenAI,
  OpenClaw,
  OpenCode,
  OpenRouter,
  Perplexity,
  Qwen,
  SambaNova,
  SiliconCloud,
  Stepfun,
  Together,
  Volcengine,
  XiaomiMiMo,
  Zhipu,
} from "@lobehub/icons";

/** Each lobehub icon has a colored `.Avatar` variant with a brand-color rounded block and logo. */
type BrandIcon = { Avatar: ComponentType<{ size: number }> };

// Map provider preset IDs to brand icons. Regional and plan suffixes share a brand.
const PROVIDER_ICONS: Record<string, BrandIcon> = {
  openai: OpenAI,
  anthropic: Anthropic,
  gemini: Gemini,
  deepseek: DeepSeek,
  glm_cn: Zhipu,
  glm: Zhipu,
  glm_coding: Zhipu,
  kimi: Kimi,
  kimi_global: Kimi,
  qwen: Qwen,
  qwen_singapore: Qwen,
  qwen_us: Qwen,
  minimax_cn: Minimax,
  minimax_global: Minimax,
  groq: Groq,
  nvidia_nim: Nvidia,
  nvidia: Nvidia,
  mistral: Mistral,
  xai: Grok,
  volcengine_ark: Volcengine,
  volcengine_ark_coding: Volcengine,
  // BytePlus has no separate logo, so use its parent brand, Volcengine.
  byteplus_ark: Volcengine,
  byteplus_ark_coding: Volcengine,
  siliconflow: SiliconCloud,
  modelscope: ModelScope,
  alibaba_model_studio: AlibabaCloud,
  tencent_hunyuan: Hunyuan,
  hugging_face: HuggingFace,
  siliconflow_global: SiliconCloud,
  together: Together,
  fireworks: Fireworks,
  deepinfra: DeepInfra,
  cerebras: Cerebras,
  sambanova: SambaNova,
  cohere: Cohere,
  github_models: Github,
  qianfan: Baidu,
  hunyuan: Hunyuan,
  stepfun: Stepfun,
  stepfun_plan: Stepfun,
  xiaomi_mimo: XiaomiMiMo,
  perplexity: Perplexity,
  novita: Novita,
  hyperbolic: Hyperbolic,
  nebius: Nebius,
  openrouter: OpenRouter,
  ollama: Ollama,
};

// Map Agent IDs to brand icons. Hermes has no @lobehub icon, so use AGENT_IMAGES.
const AGENT_ICONS: Record<string, BrandIcon> = {
  "claude-code": ClaudeCode,
  "claude-desktop": Claude,
  "gemini-cli": Gemini,
  codex: Codex,
  opencode: OpenCode,
  openclaw: OpenClaw,
};

// Agent ID to bundled bitmap logo. Store it in public/ and reference it by URL. This covers brands missing from @lobehub.
// (such as Hermes) uses it. If the file is missing, <BrandImage> falls back to an initial block and does not show a blank page.
const AGENT_IMAGES: Record<string, string> = {
  "nous-hermes-agent": "/agents/hermes.png",
};

/** Initial or abbreviation tile for items without a brand logo. */
function Fallback({ text, size }: { text: string; size: number }) {
  return (
    <span className="brand-fallback" style={{ width: size, height: size }}>
      {text}
    </span>
  );
}

/** Bundled bitmap logo that falls back to an initial tile if loading fails. */
function BrandImage({ src, fallback, size }: { src: string; fallback: string; size: number }) {
  const [failed, setFailed] = useState(false);
  if (failed) return <Fallback text={fallback} size={size} />;
  return (
    <img
      className="brand-image"
      src={src}
      alt=""
      width={size}
      height={size}
      style={{ width: size, height: size }}
      onError={() => setFailed(true)}
    />
  );
}

/** Provider brand logo that falls back to the label's initial when unmatched. */
export function ProviderIcon({ id, label, size = 28 }: { id: string; label: string; size?: number }) {
  const Icon = PROVIDER_ICONS[id];
  if (Icon) return <Icon.Avatar size={size} />;
  return <Fallback text={label.slice(0, 1).toUpperCase()} size={size} />;
}

/** Agent brand logo. Prefer bundled bitmaps, then @lobehub, then a `fallback` text block. */
export function AgentIcon({ id, fallback, size = 24 }: { id: string; fallback: string; size?: number }) {
  const image = AGENT_IMAGES[id];
  if (image) return <BrandImage src={image} fallback={fallback} size={size} />;
  const Icon = AGENT_ICONS[id];
  if (Icon) return <Icon.Avatar size={size} />;
  return <Fallback text={fallback} size={size} />;
}
