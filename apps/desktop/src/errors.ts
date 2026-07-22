/** 把结构化错误码翻成"哪层坏了 / 怎么办",而不是压成一句不知所措的字符串。 */

export interface HumanError {
  /** 坏在哪一层。 */
  layer: string;
  /** 一句话说清发生了什么。 */
  message: string;
  /** 用户下一步能做什么。 */
  suggestion: string;
}

const TABLE: Record<string, HumanError> = {
  invalid_request: {
    layer: "请求",
    message: "请求格式不合法,上游拒绝",
    suggestion: "检查客户端发的参数;多为协议不匹配",
  },
  auth: {
    layer: "鉴权 · Key",
    message: "凭证被上游拒绝",
    suggestion: "检查该 Provider 的 API Key 是否正确、未过期、有权限",
  },
  payment_required: {
    layer: "账户 · 余额",
    message: "账户余额不足,或该端点需要付费(HTTP 402)",
    suggestion: "去 Provider 那边充值 / 检查订阅额度",
  },
  rate_limit: {
    layer: "限流",
    message: "触发上游速率限制(429)",
    suggestion: "稍后重试或降低并发;已配 Retry-After 会自动等",
  },
  capacity: {
    layer: "容量",
    message: "上游暂时没有容量",
    suggestion: "稍后重试;配了多档/备用会自动 fallback",
  },
  capability: {
    layer: "能力",
    message: "模型不支持该请求(工具 / 视觉 / 多模态)",
    suggestion: "换一个支持的模型;注意本代理只路由语言模型,图片/音频会被拒",
  },
  context_length: {
    layer: "上下文",
    message: "请求超过了模型的上下文窗口",
    suggestion: "缩短输入,或换一个上下文更大的模型",
  },
  content_policy: {
    layer: "内容策略",
    message: "上游按内容策略拒绝",
    suggestion: "调整请求内容;换 Provider 通常也会拒,不会自动重试",
  },
  upstream_unavailable: {
    layer: "网络 · 上游",
    message: "上游不可达(连不上 / 传输失败)",
    suggestion: "检查 Base URL、网络、出站代理;有备用会自动 fallback",
  },
  transport_truncated: {
    layer: "网络",
    message: "连接中途断开,回答不完整",
    suggestion: "重试;这不算成功(半截流不记 200)",
  },
  provider_protocol_error: {
    layer: "协议",
    message: "上游返回了非法响应体(2xx 里塞了 error / 坏 JSON)",
    suggestion: "检查该 Provider 的协议是否与所选适配器匹配",
  },
  timeout: {
    layer: "超时",
    message: "上游在预算内没有响应",
    suggestion: "重试或换 Provider;可调请求 deadline",
  },
  internal: {
    layer: "内部",
    message: "代理内部错误",
    suggestion: "拿这条的请求 ID 看日志排查",
  },
};

/** 未知码的兜底,永远给一层 + 一句建议,不裸奔字符串。 */
export function humanizeErrorCode(code: string | null | undefined): HumanError | null {
  if (!code) return null;
  return (
    TABLE[code] ?? {
      layer: "未知",
      message: `未归类的错误码:${code}`,
      suggestion: "拿请求 ID 看日志",
    }
  );
}
