import type { Language } from "./components/LanguageProvider";

export interface HumanError {
  layer: string;
  message: string;
  suggestion: string;
}

const ENGLISH_ERROR_GUIDANCE: Record<string, HumanError> = {
  invalid_request: { layer: "Request", message: "The upstream rejected an invalid request.", suggestion: "Check the client parameters and selected protocol." },
  auth: { layer: "Authentication · Key", message: "The upstream rejected the credential.", suggestion: "Check that the provider API key is valid and has permission." },
  payment_required: { layer: "Account · Balance", message: "The account has insufficient balance or the endpoint requires payment.", suggestion: "Check the provider subscription and account credit." },
  rate_limit: { layer: "Rate limit", message: "The upstream rate limit was reached.", suggestion: "Retry later or reduce concurrency." },
  capacity: { layer: "Capacity", message: "The upstream has no available capacity right now.", suggestion: "Retry later or configure a fallback provider." },
  capability: { layer: "Capability", message: "The model does not support a capability required by this request.", suggestion: "Select a model that explicitly supports the required capability." },
  context_length: { layer: "Context", message: "The request exceeds the model context window.", suggestion: "Shorten the input or select a model with a larger context window." },
  content_policy: { layer: "Content policy", message: "The upstream rejected the request under its content policy.", suggestion: "Adjust the request content and retry." },
  upstream_unavailable: { layer: "Network · Upstream", message: "The upstream connection or transport failed.", suggestion: "Check the base URL, network, and egress settings." },
  transport_truncated: { layer: "Network", message: "The connection ended early and the response is incomplete.", suggestion: "Retry and use the request ID to inspect the upstream failure." },
  provider_protocol_error: { layer: "Protocol", message: "The upstream returned a response that does not match the protocol.", suggestion: "Check that the provider and adapter are compatible." },
  timeout: { layer: "Timeout", message: "The upstream did not finish within the request budget.", suggestion: "Retry or switch providers." },
  internal: { layer: "Local proxy", message: "The proxy failed while processing the request.", suggestion: "Copy the request ID and inspect the local logs." },
};

const CHINESE_ERROR_GUIDANCE: Record<string, HumanError> = {
  invalid_request: { layer: "请求", message: "请求格式不合法，上游拒绝处理。", suggestion: "检查客户端参数和所选协议。" },
  auth: { layer: "鉴权 · Key", message: "凭据被上游拒绝。", suggestion: "检查 Provider API Key 是否正确、有效且有权限。" },
  payment_required: { layer: "账户 · 余额", message: "账户余额不足或端点需要付费。", suggestion: "检查 Provider 订阅和账户额度。" },
  rate_limit: { layer: "限流", message: "触发上游速率限制。", suggestion: "稍后重试或降低并发。" },
  capacity: { layer: "容量", message: "上游暂时没有可用容量。", suggestion: "稍后重试，或配置备用 Provider。" },
  capability: { layer: "能力", message: "模型不支持本次请求所需能力。", suggestion: "选择明确支持该能力的语言模型。" },
  context_length: { layer: "上下文", message: "请求超过模型上下文窗口。", suggestion: "缩短输入或换用更大上下文模型。" },
  content_policy: { layer: "内容策略", message: "上游按内容策略拒绝请求。", suggestion: "调整请求内容后重试。" },
  upstream_unavailable: { layer: "网络 · 上游", message: "上游连接或传输失败。", suggestion: "检查 Base URL、网络和出站设置。" },
  transport_truncated: { layer: "网络", message: "连接中途断开，回答不完整。", suggestion: "重试请求，并结合请求 ID 排查上游。" },
  provider_protocol_error: { layer: "协议", message: "上游返回了不符合协议的响应。", suggestion: "检查 Provider 与适配器是否匹配。" },
  timeout: { layer: "超时", message: "上游未在请求预算内完成。", suggestion: "重试或切换 Provider。" },
  internal: { layer: "本地代理", message: "代理内部处理失败。", suggestion: "复制请求 ID 并查看本地日志。" },
};

export function humanizeErrorCode(
  code: string | null | undefined,
  language: Language = "en",
): HumanError | null {
  if (!code) return null;
  const guidance = language === "zh-CN" ? CHINESE_ERROR_GUIDANCE : ENGLISH_ERROR_GUIDANCE;
  return guidance[code] ?? (language === "zh-CN"
    ? {
        layer: "未知",
        message: `未归类的错误码：${code}`,
        suggestion: "复制请求 ID 并查看本地日志。",
      }
    : {
        layer: "Unknown",
        message: `Unclassified error code: ${code}`,
        suggestion: "Copy the request ID and inspect the local logs.",
      });
}
