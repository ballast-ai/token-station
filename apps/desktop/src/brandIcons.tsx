// 品牌 logo 映射:把供应商预设 id / Agent id 映射到 @lobehub/icons 的彩色
// Avatar 图标。没有官方 logo 的(如 Hermes)返回 null,调用方用首字母色块兜底。
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

/** lobehub 每个图标都带一个彩色 `.Avatar` 变体(品牌色圆角块 + logo)。 */
type BrandIcon = { Avatar: ComponentType<{ size: number }> };

// 供应商预设 id → 品牌图标。带地区/Plan 后缀的归到同一品牌。
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
  // BytePlus 无独立 logo,用其母品牌火山引擎(Volcengine)。
  byteplus_ark: Volcengine,
  byteplus_ark_coding: Volcengine,
  siliconflow: SiliconCloud,
  openrouter: OpenRouter,
  ollama: Ollama,
};

// Agent id → 品牌图标。Hermes 无官方 logo,不在表内 → 调用方兜底。
const AGENT_ICONS: Record<string, BrandIcon> = {
  "claude-code": ClaudeCode,
  codex: Codex,
  opencode: OpenCode,
  openclaw: OpenClaw,
};

/** 首字母/缩写色块,用于没有品牌 logo 的对象。 */
function Fallback({ text, size }: { text: string; size: number }) {
  return (
    <span className="brand-fallback" style={{ width: size, height: size }}>
      {text}
    </span>
  );
}

/** 供应商品牌 logo;无匹配用 label 首字母兜底。 */
export function ProviderIcon({ id, label, size = 28 }: { id: string; label: string; size?: number }) {
  const Icon = PROVIDER_ICONS[id];
  if (Icon) return <Icon.Avatar size={size} />;
  return <Fallback text={label.slice(0, 1).toUpperCase()} size={size} />;
}

/** Agent 品牌 logo;无匹配用 `fallback` 文本块兜底(如 Hermes 的「H」)。 */
export function AgentIcon({ id, fallback, size = 24 }: { id: string; fallback: string; size?: number }) {
  const Icon = AGENT_ICONS[id];
  if (Icon) return <Icon.Avatar size={size} />;
  return <Fallback text={fallback} size={size} />;
}
