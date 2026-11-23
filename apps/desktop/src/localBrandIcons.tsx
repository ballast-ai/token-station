import { useId, useMemo, type CSSProperties, type HTMLAttributes, type SVGProps } from "react";
import AlibabaCloudMono from "@lobehub/icons-static-svg/icons/alibabacloud.svg?raw";
import AlibabaCloudColor from "@lobehub/icons-static-svg/icons/alibabacloud-color.svg?raw";
import AnthropicMono from "@lobehub/icons-static-svg/icons/anthropic.svg?raw";
import BaiduMono from "@lobehub/icons-static-svg/icons/baidu.svg?raw";
import BaiduColor from "@lobehub/icons-static-svg/icons/baidu-color.svg?raw";
import CerebrasMono from "@lobehub/icons-static-svg/icons/cerebras.svg?raw";
import CerebrasColor from "@lobehub/icons-static-svg/icons/cerebras-color.svg?raw";
import ClaudeMono from "@lobehub/icons-static-svg/icons/claude.svg?raw";
import ClaudeColor from "@lobehub/icons-static-svg/icons/claude-color.svg?raw";
import ClaudeCodeMono from "@lobehub/icons-static-svg/icons/claudecode.svg?raw";
import ClaudeCodeColor from "@lobehub/icons-static-svg/icons/claudecode-color.svg?raw";
import CodexMono from "@lobehub/icons-static-svg/icons/codex.svg?raw";
import CodexColor from "@lobehub/icons-static-svg/icons/codex-color.svg?raw";
import CohereMono from "@lobehub/icons-static-svg/icons/cohere.svg?raw";
import CohereColor from "@lobehub/icons-static-svg/icons/cohere-color.svg?raw";
import CursorMono from "@lobehub/icons-static-svg/icons/cursor.svg?raw";
import DeepInfraMono from "@lobehub/icons-static-svg/icons/deepinfra.svg?raw";
import DeepInfraColor from "@lobehub/icons-static-svg/icons/deepinfra-color.svg?raw";
import DeepSeekMono from "@lobehub/icons-static-svg/icons/deepseek.svg?raw";
import DeepSeekColor from "@lobehub/icons-static-svg/icons/deepseek-color.svg?raw";
import FireworksMono from "@lobehub/icons-static-svg/icons/fireworks.svg?raw";
import FireworksColor from "@lobehub/icons-static-svg/icons/fireworks-color.svg?raw";
import GeminiMono from "@lobehub/icons-static-svg/icons/gemini.svg?raw";
import GeminiColor from "@lobehub/icons-static-svg/icons/gemini-color.svg?raw";
import GithubMono from "@lobehub/icons-static-svg/icons/github.svg?raw";
import GithubCopilotMono from "@lobehub/icons-static-svg/icons/githubcopilot.svg?raw";
import GrokMono from "@lobehub/icons-static-svg/icons/grok.svg?raw";
import GroqMono from "@lobehub/icons-static-svg/icons/groq.svg?raw";
import HunyuanMono from "@lobehub/icons-static-svg/icons/hunyuan.svg?raw";
import HunyuanColor from "@lobehub/icons-static-svg/icons/hunyuan-color.svg?raw";
import HuggingFaceMono from "@lobehub/icons-static-svg/icons/huggingface.svg?raw";
import HuggingFaceColor from "@lobehub/icons-static-svg/icons/huggingface-color.svg?raw";
import HyperbolicMono from "@lobehub/icons-static-svg/icons/hyperbolic.svg?raw";
import HyperbolicColor from "@lobehub/icons-static-svg/icons/hyperbolic-color.svg?raw";
import KimiMono from "@lobehub/icons-static-svg/icons/kimi.svg?raw";
import KimiColor from "@lobehub/icons-static-svg/icons/kimi-color.svg?raw";
import MinimaxMono from "@lobehub/icons-static-svg/icons/minimax.svg?raw";
import MinimaxColor from "@lobehub/icons-static-svg/icons/minimax-color.svg?raw";
import MistralMono from "@lobehub/icons-static-svg/icons/mistral.svg?raw";
import MistralColor from "@lobehub/icons-static-svg/icons/mistral-color.svg?raw";
import ModelScopeMono from "@lobehub/icons-static-svg/icons/modelscope.svg?raw";
import ModelScopeColor from "@lobehub/icons-static-svg/icons/modelscope-color.svg?raw";
import NebiusMono from "@lobehub/icons-static-svg/icons/nebius.svg?raw";
import NovitaMono from "@lobehub/icons-static-svg/icons/novita.svg?raw";
import NovitaColor from "@lobehub/icons-static-svg/icons/novita-color.svg?raw";
import NvidiaMono from "@lobehub/icons-static-svg/icons/nvidia.svg?raw";
import NvidiaColor from "@lobehub/icons-static-svg/icons/nvidia-color.svg?raw";
import OllamaMono from "@lobehub/icons-static-svg/icons/ollama.svg?raw";
import OpenAIMono from "@lobehub/icons-static-svg/icons/openai.svg?raw";
import OpenClawMono from "@lobehub/icons-static-svg/icons/openclaw.svg?raw";
import OpenClawColor from "@lobehub/icons-static-svg/icons/openclaw-color.svg?raw";
import OpenCodeMono from "@lobehub/icons-static-svg/icons/opencode.svg?raw";
import OpenRouterMono from "@lobehub/icons-static-svg/icons/openrouter.svg?raw";
import OpenRouterColor from "@lobehub/icons-static-svg/icons/openrouter-color.svg?raw";
import PerplexityMono from "@lobehub/icons-static-svg/icons/perplexity.svg?raw";
import PerplexityColor from "@lobehub/icons-static-svg/icons/perplexity-color.svg?raw";
import QwenMono from "@lobehub/icons-static-svg/icons/qwen.svg?raw";
import QwenColor from "@lobehub/icons-static-svg/icons/qwen-color.svg?raw";
import SambaNovaMono from "@lobehub/icons-static-svg/icons/sambanova.svg?raw";
import SambaNovaColor from "@lobehub/icons-static-svg/icons/sambanova-color.svg?raw";
import SiliconCloudMono from "@lobehub/icons-static-svg/icons/siliconcloud.svg?raw";
import SiliconCloudColor from "@lobehub/icons-static-svg/icons/siliconcloud-color.svg?raw";
import StepfunMono from "@lobehub/icons-static-svg/icons/stepfun.svg?raw";
import TogetherMono from "@lobehub/icons-static-svg/icons/together.svg?raw";
import TogetherColor from "@lobehub/icons-static-svg/icons/together-color.svg?raw";
import VolcengineMono from "@lobehub/icons-static-svg/icons/volcengine.svg?raw";
import VolcengineColor from "@lobehub/icons-static-svg/icons/volcengine-color.svg?raw";
import XiaomiMiMoMono from "@lobehub/icons-static-svg/icons/xiaomimimo.svg?raw";
import ZhipuMono from "@lobehub/icons-static-svg/icons/zhipu.svg?raw";
import ZhipuColor from "@lobehub/icons-static-svg/icons/zhipu-color.svg?raw";
import AntigravityMono from "@lobehub/icons-static-svg/icons/antigravity.svg?raw";
import AntigravityColor from "@lobehub/icons-static-svg/icons/antigravity-color.svg?raw";
import CodexAvatar from "./assets/codex-avatar.svg?raw";
import StepfunAvatar from "./assets/stepfun-avatar.svg?raw";

type GlyphProps = SVGProps<SVGSVGElement> & { size?: number | string };
interface AvatarProps extends HTMLAttributes<HTMLDivElement> {
  size: number;
  iconClassName?: string;
  iconStyle?: CSSProperties;
  iconMultiple?: number;
  shape?: "circle" | "square";
  background?: string;
  color?: string;
}

function createGlyph(source: string) {
  const root = source.match(/^<svg\b([^>]*)>([\s\S]*)<\/svg>\s*$/);
  if (!root) throw new Error("Invalid pinned brand artwork");
  const body = root[2];
  const fill = root[1].match(/\bfill="([^"]*)"/)?.[1];
  const fillRule = root[1].match(/\bfill-rule="([^"]*)"/)?.[1] as "evenodd" | "nonzero" | undefined;
  return function Glyph({ size = "1em", style, children: _children, dangerouslySetInnerHTML: _html, ...attributes }: GlyphProps) {
    const instance = useId();
    const html = useMemo(() => {
      const prefix = `brand-${instance.replace(/[^a-zA-Z0-9_-]/g, "")}-`;
      return body
        .replace(/\bid="([^"]+)"/g, (_match, identifier: string) => `id="${prefix}${identifier}"`)
        .replace(/url\(#([^)]+)\)/g, (_match, identifier: string) => `url(#${prefix}${identifier})`)
        .replace(/\b((?:xlink:)?href)="#([^"]+)"/g, (_match, attribute: string, identifier: string) => `${attribute}="#${prefix}${identifier}"`);
    }, [instance]);
    return <svg fill={fill} fillRule={fillRule} height={size} width={size} xmlns="http://www.w3.org/2000/svg"
      {...attributes} viewBox="0 0 24 24" style={{ flex: "none", lineHeight: 1, ...style }}
      dangerouslySetInnerHTML={{ __html: html }} />;
  };
}

function createBrand(source: string, preset: [string, string, number], avatarSource: string) {
  const Glyph = createGlyph(source);
  const AvatarGlyph = avatarSource === source ? Glyph : createGlyph(avatarSource);
  const title = source.match(/<title>([^<]*)<\/title>/)?.[1];
  function Avatar({ size, background = preset[0], color = preset[1], iconMultiple = preset[2],
    shape = "circle", iconClassName, iconStyle, className, style, children: _children,
    dangerouslySetInnerHTML: _html, ...attributes }: AvatarProps) {
    return <div aria-label={title} {...attributes} className={`local-brand-avatar ${className ?? ""}`}
      data-avatar-surface={background === "#000" ? "black" : background === "#fff" ? "white" : undefined}
      style={{ display: "inline-flex", alignItems: "center", justifyContent: "center", flex: "none",
        background, borderRadius: shape === "circle" ? "50%" : Math.floor(size * 0.1),
        color, width: size, height: size, overflow: "hidden", ...style }}>
      <AvatarGlyph size={size} color={color} className={iconClassName}
        style={{ transform: `scale(${iconMultiple})`, ...iconStyle }} />
    </div>;
  }
  return Object.assign(Glyph, { Avatar });
}

export const AlibabaCloud = Object.assign(createBrand(AlibabaCloudMono, ["#FF6A00","#fff",0.7], AlibabaCloudMono), { Color: createGlyph(AlibabaCloudColor) });
export const Anthropic = createBrand(AnthropicMono, ["#F1F0E8","#141413",0.75], AnthropicMono);
export const Baidu = Object.assign(createBrand(BaiduMono, ["#2932E1","#fff",0.75], BaiduMono), { Color: createGlyph(BaiduColor) });
export const Cerebras = Object.assign(createBrand(CerebrasMono, ["#F15A29","#fff",0.8], CerebrasMono), { Color: createGlyph(CerebrasColor) });
export const Claude = Object.assign(createBrand(ClaudeMono, ["#D97757","#fff",0.75], ClaudeMono), { Color: createGlyph(ClaudeColor) });
export const ClaudeCode = Object.assign(createBrand(ClaudeCodeMono, ["#09090B","#D97757",0.7], ClaudeCodeMono), { Color: createGlyph(ClaudeCodeColor) });
export const Codex = Object.assign(createBrand(CodexMono, ["#fff","#fff",0.7], CodexAvatar), { Color: createGlyph(CodexColor) });
export const Cohere = Object.assign(createBrand(CohereMono, ["#39594D","#fff",0.6], CohereMono), { Color: createGlyph(CohereColor) });
export const Cursor = createBrand(CursorMono, ["#000","#fff",0.6], CursorMono);
export const DeepInfra = Object.assign(createBrand(DeepInfraMono, ["#fff","#fff",0.75], DeepInfraColor), { Color: createGlyph(DeepInfraColor) });
export const DeepSeek = Object.assign(createBrand(DeepSeekMono, ["#4D6BFE","#fff",0.75], DeepSeekMono), { Color: createGlyph(DeepSeekColor) });
export const Fireworks = Object.assign(createBrand(FireworksMono, ["#5019C5","#000",0.75], FireworksMono), { Color: createGlyph(FireworksColor) });
export const Gemini = Object.assign(createBrand(GeminiMono, ["#fff","#fff",0.8], GeminiColor), { Color: createGlyph(GeminiColor) });
export const Github = createBrand(GithubMono, ["#000","#fff",0.75], GithubMono);
export const GithubCopilot = createBrand(GithubCopilotMono, ["#000","#fff",0.75], GithubCopilotMono);
export const Grok = createBrand(GrokMono, ["#000","#fff",0.75], GrokMono);
export const Groq = createBrand(GroqMono, ["#F55036","#fff",0.75], GroqMono);
export const Hunyuan = Object.assign(createBrand(HunyuanMono, ["#0053e0","#fff",0.75], HunyuanMono), { Color: createGlyph(HunyuanColor) });
export const HuggingFace = Object.assign(createBrand(HuggingFaceMono, ["#fff","#fff",0.75], HuggingFaceColor), { Color: createGlyph(HuggingFaceColor) });
export const Hyperbolic = Object.assign(createBrand(HyperbolicMono, ["#594CE9","#fff",0.6], HyperbolicMono), { Color: createGlyph(HyperbolicColor) });
export const Kimi = Object.assign(createBrand(KimiMono, ["#000","#fff",0.6], KimiColor), { Color: createGlyph(KimiColor) });
export const Minimax = Object.assign(createBrand(MinimaxMono, ["linear-gradient(to right, #E2167E,  #FE603C)","#fff",0.75], MinimaxMono), { Color: createGlyph(MinimaxColor) });
export const Mistral = Object.assign(createBrand(MistralMono, ["#FA520F","#fff",0.75], MistralMono), { Color: createGlyph(MistralColor) });
export const ModelScope = Object.assign(createBrand(ModelScopeMono, ["#624AFF","#fff",0.75], ModelScopeMono), { Color: createGlyph(ModelScopeColor) });
export const Nebius = createBrand(NebiusMono, ["#DAFF33","#052b42",0.6], NebiusMono);
export const Novita = Object.assign(createBrand(NovitaMono, ["#23D57C","#000",0.75], NovitaMono), { Color: createGlyph(NovitaColor) });
export const Nvidia = Object.assign(createBrand(NvidiaMono, ["#74B71B","#fff",0.75], NvidiaMono), { Color: createGlyph(NvidiaColor) });
export const Ollama = createBrand(OllamaMono, ["#fff","#000",0.75], OllamaMono);
export const OpenAI = createBrand(OpenAIMono, ["#000","#fff",0.75], OpenAIMono);
export const OpenClaw = Object.assign(createBrand(OpenClawMono, ["#000","#fff",0.75], OpenClawColor), { Color: createGlyph(OpenClawColor) });
export const OpenCode = createBrand(OpenCodeMono, ["#000","#fff",0.75], OpenCodeMono);
export const OpenRouter = Object.assign(createBrand(OpenRouterMono, ["#000","#C8FF00",0.75], OpenRouterMono), { Color: createGlyph(OpenRouterColor) });
export const Perplexity = Object.assign(createBrand(PerplexityMono, ["#22B8CD","#000",0.75], PerplexityMono), { Color: createGlyph(PerplexityColor) });
export const Qwen = Object.assign(createBrand(QwenMono, ["#615ced","#fff",0.75], QwenMono), { Color: createGlyph(QwenColor) });
export const SambaNova = Object.assign(createBrand(SambaNovaMono, ["#EE7624","#fff",0.6], SambaNovaMono), { Color: createGlyph(SambaNovaColor) });
export const SiliconCloud = Object.assign(createBrand(SiliconCloudMono, ["#6E29F6","#fff",0.7], SiliconCloudMono), { Color: createGlyph(SiliconCloudColor) });
export const Stepfun = createBrand(StepfunMono, ["#fff","#000",0.65], StepfunAvatar);
export const Together = Object.assign(createBrand(TogetherMono, ["#fff","#000",0.75], TogetherColor), { Color: createGlyph(TogetherColor) });
export const Volcengine = Object.assign(createBrand(VolcengineMono, ["#fff","#fff",0.75], VolcengineColor), { Color: createGlyph(VolcengineColor) });
export const XiaomiMiMo = createBrand(XiaomiMiMoMono, ["#000","#fff",0.7], XiaomiMiMoMono);
export const Zhipu = Object.assign(createBrand(ZhipuMono, ["#3859FF","#fff",0.75], ZhipuMono), { Color: createGlyph(ZhipuColor) });
export const Antigravity = Object.assign(createBrand(AntigravityMono, ["#fff","#fff",0.7], AntigravityColor), { Color: createGlyph(AntigravityColor) });
