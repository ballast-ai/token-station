// Brand logo map: map provider preset IDs and Agent IDs to colored @lobehub/icons
// Avatar icon. Return null for brands without an official logo, such as Hermes. The caller uses a colored initial block.
import type { ComponentType } from "react";
import {
  ClaudeCode,
  Codex,
  DeepSeek,
  Grok,
  Groq,
  Kimi,
  Minimax,
  Mistral,
  Nvidia,
  Ollama,
  OpenAI,
  OpenClaw,
  OpenCode,
  OpenRouter,
  Qwen,
  SiliconCloud,
  Volcengine,
  Zhipu,
} from "@lobehub/icons";

/** Each lobehub icon has a colored `.Avatar` variant with a brand-color rounded block and logo. */
type BrandIcon = { Avatar: ComponentType<{ size: number }> };

// Map provider preset IDs to brand icons. Regional and plan suffixes share a brand.
const PROVIDER_ICONS: Record<string, BrandIcon> = {
  openai: OpenAI,
  deepseek: DeepSeek,
  glm_cn: Zhipu,
  glm: Zhipu,
  glm_coding: Zhipu,
  kimi: Kimi,
  kimi_global: Kimi,
  qwen: Qwen,
  qwen_us: Qwen,
  minimax_cn: Minimax,
  minimax_global: Minimax,
  groq: Groq,
  nvidia_nim: Nvidia,
  mistral: Mistral,
  xai: Grok,
  volcengine_ark: Volcengine,
  volcengine_ark_coding: Volcengine,
  // BytePlus has no separate logo, so use its parent brand, Volcengine.
  byteplus_ark: Volcengine,
  byteplus_ark_coding: Volcengine,
  siliconflow: SiliconCloud,
  openrouter: OpenRouter,
  ollama: Ollama,
};

// Agent ID to brand icon. Hermes has no official logo and is not in the map. The caller provides the fallback.
const AGENT_ICONS: Record<string, BrandIcon> = {
  "claude-code": ClaudeCode,
  codex: Codex,
  opencode: OpenCode,
  openclaw: OpenClaw,
};

/** Initial or abbreviation tile for items without a brand logo. */
function Fallback({ text, size }: { text: string; size: number }) {
  return (
    <span className="brand-fallback" style={{ width: size, height: size }}>
      {text}
    </span>
  );
}

/** Provider brand logo that falls back to the label's initial when unmatched. */
export function ProviderIcon({ id, label, size = 28 }: { id: string; label: string; size?: number }) {
  const Icon = PROVIDER_ICONS[id];
  if (Icon) return <Icon.Avatar size={size} />;
  return <Fallback text={label.slice(0, 1).toUpperCase()} size={size} />;
}

/** Agent brand logo. If no match exists, use a `fallback` text block, such as H for Hermes. */
export function AgentIcon({ id, fallback, size = 24 }: { id: string; fallback: string; size?: number }) {
  const Icon = AGENT_ICONS[id];
  if (Icon) return <Icon.Avatar size={size} />;
  return <Fallback text={fallback} size={size} />;
}
