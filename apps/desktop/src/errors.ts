export interface HumanError {
  layer: string;
  message: string;
  suggestion: string;
}

const ERROR_GUIDANCE: Record<string, HumanError> = {
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

export function humanizeErrorCode(code: string | null | undefined): HumanError | null {
  if (!code) return null;
  return ERROR_GUIDANCE[code] ?? {
    layer: "未知",
    message: `未归类的错误码：${code}`,
    suggestion: "复制请求 ID 并查看本地日志。",
  };
}
