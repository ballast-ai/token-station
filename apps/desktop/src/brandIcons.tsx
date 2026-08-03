// 品牌 logo 映射:把供应商预设 id / Agent id 映射到 @lobehub/icons 的彩色
// Avatar 图标。没有官方 logo 的(如 Hermes)返回 null,调用方用首字母色块兜底。
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

type BrandGlyph = ComponentType<{ size: number }>;

/** lobehub 图标本体可直接渲染;多数品牌另带 `.Color`,全部品牌都带 `.Avatar`。 */
type BrandIcon = BrandGlyph & {
  Avatar: BrandGlyph;
  Color?: BrandGlyph;
};

// 供应商预设 id → 品牌图标。带地区/Plan 后缀的归到同一品牌。
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
  // BytePlus 无独立 logo,用其母品牌火山引擎(Volcengine)。
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

// Agent id → 品牌图标。Hermes(Nous Hermes)无 @lobehub logo → 走 AGENT_IMAGES 自带图。
const AGENT_ICONS: Record<string, BrandIcon> = {
  "claude-code": ClaudeCode,
  "claude-desktop": Claude,
  "gemini-cli": Gemini,
  codex: Codex,
  opencode: OpenCode,
  openclaw: OpenClaw,
};

// Agent id → 自带位图 logo(放在 public/ 下,按 URL 引用)。@lobehub 没有的品牌
// (如 Hermes)用它;文件缺失时 <BrandImage> 自动回退到首字母块,不会白屏。
const AGENT_IMAGES: Record<string, string> = {
  "nous-hermes-agent": "/agents/hermes.png",
};

/** 首字母/缩写色块,用于没有品牌 logo 的对象。 */
function Fallback({ text, size }: { text: string; size: number }) {
  return (
    <span className="brand-fallback" style={{ width: size, height: size }}>
      {text}
    </span>
  );
}

/** 自带位图 logo;加载失败(文件未放置)时回退到首字母块。 */
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

/** 供应商品牌 logo;无匹配用 label 首字母兜底。 */
export function ProviderIcon({ id, label, size = 28 }: { id: string; label: string; size?: number }) {
  const Icon = PROVIDER_ICONS[id];
  if (Icon) return <Icon.Avatar size={size} />;
  return <Fallback text={label.slice(0, 1).toUpperCase()} size={size} />;
}

/** Agent 品牌 logo;使用完整品牌图形而非黑底 Avatar,避免标记被缩成角落小点。 */
export function AgentIcon({ id, fallback, size = 24 }: { id: string; fallback: string; size?: number }) {
  const image = AGENT_IMAGES[id];
  const Icon = AGENT_ICONS[id];
  const Glyph = Icon ? (Icon.Color ?? Icon) : null;
  return (
    <span
      className="agent-brand-glyph"
      data-agent-brand={id}
      style={{ width: size, height: size }}
    >
      {image ? (
        <BrandImage src={image} fallback={fallback} size={size} />
      ) : Glyph ? (
        <Glyph size={size} />
      ) : (
        <Fallback text={fallback} size={size} />
      )}
    </span>
  );
}
