import type { Language } from "./components/LanguageProvider";
import type { ReceiptConversionView, ReceiptView } from "./api";

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

function localConversionMessage(
  conversion: ReceiptConversionView,
  language: Language,
): HumanError {
  const chinese = language === "zh-CN";
  const reason = conversion.reason_code;
  const detail = conversion.reason_detail;
  const cause = (() => {
    if (reason === "invalid_json") return chinese ? "请求 JSON 无法解析。" : "The request JSON could not be parsed.";
    if (reason === "provider_tool_unsupported") return chinese
      ? detail === "web_search"
        ? "当前翻译链路不能执行服务端 WebSearch。"
        : "当前翻译链路不能执行请求中的供应商托管工具。"
      : detail === "web_search"
        ? "The translated route cannot execute provider-hosted Web Search."
        : "The translated route cannot execute the requested provider-hosted tool.";
    if (reason === "unsupported_tool_type") return chinese ? "请求包含不支持的工具类型。" : "The request contains an unsupported tool type.";
    if (reason === "structured_output") return chinese ? "当前链路不能保留请求的结构化输出语义。" : "The route cannot preserve the requested structured-output semantics.";
    if (reason === "unsupported_media") return chinese ? "当前链路不支持请求中的媒体类型。" : "The route does not support a media type in the request.";
    if (reason === "invalid_protocol_shape") return chinese ? "请求字段缺失或结构不符合所选协议。" : "Required fields are missing or the request shape does not match the selected protocol.";
    return chinese ? "请求无法转换为 Token Station 的内部格式。" : "The request could not be converted to Token Station's internal format.";
  })();
  return chinese
    ? {
        layer: "本地请求转换",
        message: `${cause} 请求尚未发往上游。`,
        suggestion: reason === "provider_tool_unsupported"
          ? "改用真正支持该托管工具的原生 Provider，或关闭该工具。"
          : "检查 Agent 请求协议和必填字段，然后重试。",
      }
    : {
        layer: "Local request conversion",
        message: `${cause} The request was not sent upstream.`,
        suggestion: reason === "provider_tool_unsupported"
          ? "Use a native provider that supports the hosted tool, or disable that tool."
          : "Check the Agent protocol and required fields, then retry.",
      };
}

export function humanizeReceiptError(
  receipt: ReceiptView,
  language: Language = "en",
): HumanError | null {
  const failedInbound = receipt.conversion_reports.find(
    (conversion) => conversion.stage === "inbound_normalize" && !conversion.succeeded,
  );
  const noUpstreamAttempt = receipt.attempts === 0 && receipt.attempt_records.length === 0;
  if (failedInbound && noUpstreamAttempt && receipt.decision == null) {
    return localConversionMessage(failedInbound, language);
  }
  return humanizeErrorCode(receipt.error_code, language);
}

interface LocalizedAppError {
  matches: RegExp;
  en: string;
  zh: string;
}

interface LocalizedAppMessage {
  en: string;
  zh: string;
}

function modelContractGuidance(error: unknown): LocalizedAppMessage | null {
  if (!error || typeof error !== "object") return null;
  const value = error as { code?: unknown; target?: unknown };
  const code = typeof value.code === "string" ? value.code : "";
  const target = typeof value.target === "string" && value.target.length <= 256
    ? `\`${value.target}\``
    : null;
  const subject = target ?? "the selected model";
  const zhSubject = target ?? "当前模型";
  const guidance: Record<string, LocalizedAppMessage> = {
    agent_runtime_transition: {
      en: "The proxy is switching runtime instances. Wait for the transition to finish; OpenCode readiness will then be checked again.",
      zh: "代理正在切换运行实例。请等待切换完成，届时将重新检查 OpenCode 接入条件。",
    },
    model_contract_exact_routing_unsupported: {
      en: "OpenCode's fixed model is incompatible with exact-model routing. Switch this Agent to tiered, quota-first, or direct routing.",
      zh: "OpenCode 固定模型与精确模型路由不兼容。请将该 Agent 切换为分层、额度优先或单独路由。",
    },
    model_contract_invalid_route: {
      en: "The OpenCode route is invalid. Repair and save the route configuration before connecting.",
      zh: "OpenCode 路由配置无效。请修复并保存路由配置后再接入。",
    },
    model_contract_no_reachable_model: {
      en: "The current OpenCode route has no reachable model. Add a reachable provider and model, then save the route.",
      zh: "OpenCode 当前路由没有可达模型。请添加可达的供应商和模型，然后保存路由。",
    },
    model_contract_unknown_provider: {
      en: `The OpenCode route references unknown provider ${subject}. Repair the route or restore that provider.`,
      zh: `OpenCode 路由引用了未知供应商 ${zhSubject}。请修复路由或恢复该供应商。`,
    },
    model_contract_unknown_model: {
      en: `The OpenCode route references unknown model ${subject}. Repair the route or add that model.`,
      zh: `OpenCode 路由引用了未知模型 ${zhSubject}。请修复路由或添加该模型。`,
    },
    model_contract_missing_context_window: {
      en: `Model ${subject} has no context-window limit. Complete this model's limits in Providers, then restart the proxy.`,
      zh: `模型 ${zhSubject} 缺少上下文上限。请前往供应商页面完善该模型限制，然后重启代理。`,
    },
    model_contract_missing_max_output_tokens: {
      en: `Model ${subject} has no maximum output token limit. Complete this model's limits in Providers, then restart the proxy.`,
      zh: `模型 ${zhSubject} 缺少最大输出 Token 上限。请前往供应商页面完善该模型限制，然后重启代理。`,
    },
    model_contract_invalid_limits: {
      en: `Model ${subject} has invalid token limits. Set maximum output below its context window, then restart the proxy.`,
      zh: `模型 ${zhSubject} 的 Token 上限无效。请将最大输出设为小于上下文上限，然后重启代理。`,
    },
  };
  return guidance[code] ?? null;
}

function routerConfigGuidance(raw: string): LocalizedAppMessage | null {
  const emptyPool = raw.match(/pool `([^`\r\n]{1,128})` has no members/i);
  if (emptyPool) {
    const pool = emptyPool[1];
    return {
      en: `Route pool \`${pool}\` is empty. Add a provider and model to this pool, then save again.`,
      zh: `路由池 \`${pool}\` 为空。请为该路由池添加供应商和模型，然后重新保存。`,
    };
  }

  const unknownPool = raw.match(
    /([^\r\n]{1,128}?) routes to pool `([^`\r\n]{1,128})`, which does not exist/i,
  );
  if (unknownPool) {
    const reference = unknownPool[1].trim();
    const pool = unknownPool[2];
    const rule = reference.match(/^rule `([^`\r\n]{1,128})`$/i);
    const englishReference = rule ? `Rule \`${rule[1]}\`` : `Configuration field \`${reference}\``;
    const chineseReference = rule ? `规则 \`${rule[1]}\`` : `配置字段 \`${reference}\``;
    return {
      en: `${englishReference} points to missing route pool \`${pool}\`. Choose an existing pool for this ${rule ? "rule" : "field"}, then save again.`,
      zh: `${chineseReference} 指向不存在的路由池 \`${pool}\`。请为该${rule ? "规则" : "字段"}选择现有路由池，然后重新保存。`,
    };
  }

  return null;
}

const APP_ERROR_GUIDANCE: LocalizedAppError[] = [
  {
    matches: /apply_in_progress|configuration update.*(?:progress|running)|配置.*(?:正在应用|处理中)/i,
    en: "Another configuration update is still running. Wait a moment, then try again.",
    zh: "另一项配置更新仍在进行。请稍等片刻，然后重试。",
  },
  {
    matches: /a router with no pools can route nothing|没有设置路由池|no route pools?|routing.*not configured/i,
    en: "Routing is not configured yet. Select a provider and model for at least one route, then save before starting Token Station.",
    zh: "路由尚未配置。请至少为一个路由选择供应商和模型，保存后再启动 Token Station。",
  },
  {
    matches: /路由池.*(?:缺少|未配置)|route pool.*(?:missing|incomplete)|tier.*(?:missing|unconfigured)/i,
    en: "A route is incomplete. Select both a provider and a model for that route, then save again.",
    zh: "有一个路由尚未配置完整。请同时选择供应商和模型，然后重新保存。",
  },
  {
    matches: /unknown (?:provider|upstream)|未知供应商|未知上游|provider.*not found/i,
    en: "The selected provider is no longer available. Choose an existing provider and save again.",
    zh: "所选供应商已不可用。请选择现有供应商，然后重新保存。",
  },
  {
    matches: /unknown model|未知模型|model.*not found|模型.*不存在/i,
    en: "The selected model is no longer available. Refresh the model list and choose another model.",
    zh: "所选模型已不可用。请刷新模型列表，然后选择其他模型。",
  },
  {
    matches: /unknown agent|未知 Agent|agent.*not found|计划不存在或已消费/i,
    en: "This Agent is no longer available or its setup has expired. Rescan Agents and start the setup again.",
    zh: "这个 Agent 已不可用，或接入操作已经过期。请重新扫描 Agent，然后再次开始接入。",
  },
  {
    matches: /缺少供应商和模型|(?:high|mid|low|上|中|下).*档.*(?:缺少|未配置)/i,
    en: "A route is incomplete. Select both a provider and a model for that route, then save again.",
    zh: "有一个路由尚未配置完整。请同时选择供应商和模型，然后重新保存。",
  },
  {
    matches: /credential|api[ _-]?key|auth(?:entication|orization)?|鉴权|凭据|密钥/i,
    en: "The credential could not be used. Check the API key and its permissions, then try again.",
    zh: "凭据无法使用。请检查 API Key 及其权限，然后重试。",
  },
  {
    matches: /open_external_failed|could not open.*(?:page|url)|无法打开外部页面/i,
    en: "The external page could not be opened. Check the URL and your default browser, then try again.",
    zh: "无法打开外部页面。请检查网址和默认浏览器，然后重试。",
  },
  {
    matches: /Agent .*model metadata refresh failed|Agent 模型元数据刷新失败/i,
    en: "The proxy is running, but Token Station could not refresh one managed Agent configuration. Open the Agent page, rescan, and repair that Agent before using its route.",
    zh: "代理已经运行，但一个已接管 Agent 的模型元数据刷新失败。请打开 Agent 页面重新扫描，并修复该 Agent 后再使用它的路由。",
  },
  {
    matches: /timed? ?out|timeout|超时/i,
    en: "The operation took too long and was stopped. Check the network connection, then try again.",
    zh: "操作等待时间过长，已停止。请检查网络连接，然后重试。",
  },
  {
    matches: /network|\bconnect(?:ion)?\b|dns|tls|certificate|request failed|网络|连接失败|证书/i,
    en: "Token Station could not reach the service. Check the network, Base URL, and proxy settings, then try again.",
    zh: "Token Station 无法连接到服务。请检查网络、Base URL 和代理设置，然后重试。",
  },
  {
    matches: /invalid.*(?:url|address)|proxy address|代理地址无效|地址.*不合法/i,
    en: "The address is not valid. Enter a complete HTTP, HTTPS, or SOCKS5 address and try again.",
    zh: "地址格式不正确。请输入完整的 HTTP、HTTPS 或 SOCKS5 地址，然后重试。",
  },
  {
    matches: /address already in use|port.*(?:used|busy)|listener|listen_restore|端口.*占用|监听/i,
    en: "The local proxy could not use its configured address. Close the app using that port or choose another port, then restart the proxy.",
    zh: "本地代理无法使用当前监听地址。请关闭占用该端口的应用，或换一个端口，然后重启代理。",
  },
  {
    matches: /proxy.*not running|代理未运行/i,
    en: "The local proxy is not running. Start the proxy, then try again.",
    zh: "本地代理尚未运行。请先启动代理，然后重试。",
  },
  {
    matches: /generation|proxy.*(?:start|restart)|代理启动|启动代理/i,
    en: "The local proxy could not restart safely. Quit and reopen Token Station, then try again.",
    zh: "本地代理无法安全重启。请退出并重新打开 Token Station，然后重试。",
  },
  {
    matches: /state_poisoned|app_state_poisoned|写锁已损坏|状态不可用|operation_.*poisoned/i,
    en: "Token Station's local state is temporarily unavailable. Restart the app; your saved configuration will remain unchanged.",
    zh: "Token Station 的本地状态暂时不可用。请重启应用，已保存的配置不会改变。",
  },
  {
    matches: /resource limit|资源上限|response.*(?:too large|MiB)|超过.*限制/i,
    en: "The service returned more data than Token Station can safely process. Narrow the request or try again later.",
    zh: "服务返回的数据超过 Token Station 的安全处理上限。请缩小请求范围，或稍后重试。",
  },
  {
    matches: /model_providers|configuration format|config(?:uration)? file|配置格式|配置结构|JSON5|TOML|YAML/i,
    en: "The configuration file has a format Token Station cannot safely edit. Fix the file syntax or restore a known-good backup, then rescan.",
    zh: "配置文件格式无法安全编辑。请修复文件语法，或恢复可用备份，然后重新扫描。",
  },
  {
    matches: /baseline|before_hash|revision_hash|前置值|基线快照|revision.*changed|配置.*已变化/i,
    en: "The configuration changed after it was opened. Reload the latest version, review it, and apply the change again.",
    zh: "配置在打开后又发生了变化。请重新加载最新版本，确认后再次应用。",
  },
  {
    matches: /permission denied|read-only file|failed to (?:read|write)|无法(?:读取|写入)|权限/i,
    en: "Token Station could not access the required local file. Check file permissions and available disk space, then try again.",
    zh: "Token Station 无法访问所需的本地文件。请检查文件权限和磁盘空间，然后重试。",
  },
  {
    matches: /database|sqlite|schema|指标库|数据库/i,
    en: "The local data store could not be opened safely. Use Recovery mode to inspect or export the local data.",
    zh: "本地数据无法安全打开。请使用自救模式检查或导出本地数据。",
  },
  {
    matches: /model_catalog_provider_required/i,
    en: "Enter a provider name and Base URL before loading its model catalog.",
    zh: "请先填写供应商名称和 Base URL，再读取模型目录。",
  },
  {
    matches: /model_catalog_reference_requires_save/i,
    en: "Save the environment-variable or file credential reference before loading the model catalog.",
    zh: "env/file 只保存凭据引用。请先保存供应商，再读取模型目录。",
  },
  {
    matches: /model_catalog_api_key_required/i,
    en: "Enter an API key before loading this provider's model catalog.",
    zh: "请先填写 API Key，再读取该供应商的模型目录。",
  },
  {
    matches: /quota|usage|pricing|catalog|额度|用量|价格目录|模型目录/i,
    en: "The latest provider data is unavailable. Keep the current settings and try refreshing again later.",
    zh: "暂时无法获取最新的供应商数据。请保留当前设置，稍后再次刷新。",
  },
  {
    matches: /暂无公开发布版本|no public release (?:is )?available/i,
    en: "No public release is available yet. Check again later or open the Releases page.",
    zh: "暂无公开发布版本。请稍后重试，或打开发布页查看。",
  },
  {
    matches: /官方更新公钥|official update public key/i,
    en: "This build cannot install updates because it does not include the official update public key. Download the app from the Releases page instead.",
    zh: "当前构建未内置官方更新公钥，无法在 App 内安装更新。请改从发布页下载安装。",
  },
  {
    matches: /update_version_changed:/i,
    en: "A newer update became available. Check again before installing.",
    zh: "可用更新已经发生变化。请重新检查后再确认安装。",
  },
  {
    matches: /update_expected_version_missing:/i,
    en: "The selected update is no longer available. Check again before installing.",
    zh: "之前确认的更新已不可用。请重新检查后再确认安装。",
  },
  {
    matches: /update|upgrade|release|检查更新|安装更新|应用更新|可用版本|发布版本/i,
    en: "Token Station could not check for updates. Check the network connection and try again later.",
    zh: "Token Station 无法检查更新。请检查网络连接，稍后重试。",
  },
];

function appErrorText(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  if (error && typeof error === "object") {
    const value = error as {
      code?: unknown;
      message?: unknown;
      suggestion?: unknown;
      detail?: unknown;
    };
    return [value.code, value.message, value.suggestion, value.detail]
      .filter((part) => part !== null && part !== undefined)
      .map(String)
      .join(" ");
  }
  return String(error ?? "");
}

function selectedAppLanguage(language?: Language): Language {
  if (language) return language;
  try {
    return window.localStorage.getItem("token-station-language") === "zh-CN" ? "zh-CN" : "en";
  } catch {
    return "en";
  }
}

export function humanizeAppError(error: unknown, language?: Language): string {
  const raw = appErrorText(error);
  const modelGuidance = modelContractGuidance(error);
  const routerGuidance = routerConfigGuidance(raw);
  const guidance = APP_ERROR_GUIDANCE.find((item) => item.matches.test(raw));
  const chinese = selectedAppLanguage(language) === "zh-CN";
  if (modelGuidance) return chinese ? modelGuidance.zh : modelGuidance.en;
  if (routerGuidance) return chinese ? routerGuidance.zh : routerGuidance.en;
  if (guidance) return chinese ? guidance.zh : guidance.en;
  return chinese
    ? "操作未能完成。请重试；如果仍然失败，请从自救模式打开本地日志。"
    : "The operation could not be completed. Try again. If it still fails, open the local logs from Recovery mode.";
}
