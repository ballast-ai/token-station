import { getActiveLanguage, localizedCopy, type Language } from "./i18n";
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

const TRADITIONAL_CHINESE_ERROR_GUIDANCE: Record<string, HumanError> = {
  invalid_request: { layer: "請求", message: "請求格式無效，上游拒絕處理。", suggestion: "檢查客戶端參數和所選協定。" },
  auth: { layer: "驗證 · Key", message: "上游拒絕此憑證。", suggestion: "檢查 Provider API Key 是否有效且具有權限。" },
  payment_required: { layer: "帳戶 · 餘額", message: "帳戶餘額不足或端點需要付費。", suggestion: "檢查 Provider 訂閱和帳戶額度。" },
  rate_limit: { layer: "速率限制", message: "已達到上游速率限制。", suggestion: "稍後重試或降低並行數。" },
  capacity: { layer: "容量", message: "上游目前沒有可用容量。", suggestion: "稍後重試或設定備援 Provider。" },
  capability: { layer: "能力", message: "模型不支援此請求所需的能力。", suggestion: "選擇明確支援該能力的模型。" },
  context_length: { layer: "上下文", message: "請求超過模型的上下文視窗。", suggestion: "縮短輸入或選擇具有更大上下文視窗的模型。" },
  content_policy: { layer: "內容政策", message: "上游依內容政策拒絕此請求。", suggestion: "調整請求內容後重試。" },
  upstream_unavailable: { layer: "網路 · 上游", message: "上游連線或傳輸失敗。", suggestion: "檢查 Base URL、網路和出口設定。" },
  transport_truncated: { layer: "網路", message: "連線提前結束，回應不完整。", suggestion: "重試並使用請求 ID 檢查上游失敗。" },
  provider_protocol_error: { layer: "協定", message: "上游回應不符合協定。", suggestion: "檢查 Provider 與介面卡是否相容。" },
  timeout: { layer: "逾時", message: "上游未在請求時間內完成。", suggestion: "重試或切換 Provider。" },
  internal: { layer: "本機代理", message: "代理處理請求時失敗。", suggestion: "複製請求 ID 並檢查本機記錄。" },
};

const JAPANESE_ERROR_GUIDANCE: Record<string, HumanError> = {
  invalid_request: { layer: "リクエスト", message: "無効なリクエストがアップストリームに拒否されました。", suggestion: "クライアントのパラメーターと選択したプロトコルを確認してください。" },
  auth: { layer: "認証 · Key", message: "アップストリームが認証情報を拒否しました。", suggestion: "Provider API Key が有効で、必要な権限があることを確認してください。" },
  payment_required: { layer: "アカウント · 残高", message: "アカウントの残高が不足しているか、エンドポイントが支払いを要求しています。", suggestion: "Provider の契約とアカウント残高を確認してください。" },
  rate_limit: { layer: "レート制限", message: "アップストリームのレート制限に達しました。", suggestion: "後でもう一度試すか、同時実行数を減らしてください。" },
  capacity: { layer: "容量", message: "アップストリームに現在利用可能な容量がありません。", suggestion: "後でもう一度試すか、フォールバック Provider を設定してください。" },
  capability: { layer: "機能", message: "モデルはこのリクエストに必要な機能をサポートしていません。", suggestion: "必要な機能を明示的にサポートするモデルを選択してください。" },
  context_length: { layer: "コンテキスト", message: "リクエストがモデルのコンテキストウィンドウを超えています。", suggestion: "入力を短くするか、より大きなコンテキストウィンドウを持つモデルを選択してください。" },
  content_policy: { layer: "コンテンツポリシー", message: "アップストリームがコンテンツポリシーによりリクエストを拒否しました。", suggestion: "リクエスト内容を調整して再試行してください。" },
  upstream_unavailable: { layer: "ネットワーク · アップストリーム", message: "アップストリームへの接続または転送に失敗しました。", suggestion: "Base URL、ネットワーク、送信設定を確認してください。" },
  transport_truncated: { layer: "ネットワーク", message: "接続が途中で終了し、応答が不完全です。", suggestion: "再試行し、リクエスト ID を使用してアップストリーム障害を確認してください。" },
  provider_protocol_error: { layer: "プロトコル", message: "アップストリームの応答がプロトコルに一致しません。", suggestion: "Provider とアダプターに互換性があることを確認してください。" },
  timeout: { layer: "タイムアウト", message: "アップストリームが制限時間内に完了しませんでした。", suggestion: "再試行するか、Provider を切り替えてください。" },
  internal: { layer: "ローカルプロキシ", message: "プロキシがリクエストの処理中に失敗しました。", suggestion: "リクエスト ID をコピーしてローカルログを確認してください。" },
};

export function humanizeErrorCode(
  code: string | null | undefined,
  language: Language = "en",
): HumanError | null {
  if (!code) return null;
  const guidance = {
    en: ENGLISH_ERROR_GUIDANCE,
    "zh-CN": CHINESE_ERROR_GUIDANCE,
    "zh-TW": TRADITIONAL_CHINESE_ERROR_GUIDANCE,
    ja: JAPANESE_ERROR_GUIDANCE,
  }[language];
  return guidance[code] ?? {
    layer: localizedCopy(language, "Unknown", "未知", "未知", "不明"),
    message: localizedCopy(
      language,
      `Unclassified error code: ${code}`,
      `未归类的错误码：${code}`,
      `未分類的錯誤碼：${code}`,
      `未分類のエラーコード：${code}`,
    ),
    suggestion: localizedCopy(
      language,
      "Copy the request ID and inspect the local logs.",
      "复制请求 ID 并查看本地日志。",
      "複製請求 ID 並檢查本機記錄。",
      "リクエスト ID をコピーしてローカルログを確認してください。",
    ),
  };
}

function localConversionMessage(
  conversion: ReceiptConversionView,
  language: Language,
): HumanError {
  const reason = conversion.reason_code;
  const detail = conversion.reason_detail;
  const cause = (() => {
    if (reason === "invalid_json") return localizedCopy(
      language,
      "The request JSON could not be parsed.",
      "请求 JSON 无法解析。",
      "無法解析請求 JSON。",
      "リクエスト JSON を解析できませんでした。",
    );
    if (reason === "provider_tool_unsupported") return detail === "web_search"
      ? localizedCopy(
          language,
          "The translated route cannot execute provider-hosted Web Search.",
          "当前翻译链路不能执行服务端 WebSearch。",
          "目前的轉譯路由無法執行 Provider 託管的 Web Search。",
          "変換されたルートでは Provider 側の Web Search を実行できません。",
        )
      : localizedCopy(
          language,
          "The translated route cannot execute the requested provider-hosted tool.",
          "当前翻译链路不能执行请求中的供应商托管工具。",
          "目前的轉譯路由無法執行請求中的 Provider 託管工具。",
          "変換されたルートでは、要求された Provider 側のツールを実行できません。",
        );
    if (reason === "unsupported_tool_type") return localizedCopy(
      language,
      "The request contains an unsupported tool type.",
      "请求包含不支持的工具类型。",
      "請求包含不支援的工具類型。",
      "リクエストにサポートされていないツール種別が含まれています。",
    );
    if (reason === "structured_output") return localizedCopy(
      language,
      "The route cannot preserve the requested structured-output semantics.",
      "当前链路不能保留请求的结构化输出语义。",
      "目前的路由無法保留請求的結構化輸出語意。",
      "このルートでは要求された構造化出力の意味を保持できません。",
    );
    if (reason === "unsupported_media") return localizedCopy(
      language,
      "The route does not support a media type in the request.",
      "当前链路不支持请求中的媒体类型。",
      "目前的路由不支援請求中的媒體類型。",
      "このルートはリクエスト内のメディア種別をサポートしていません。",
    );
    if (reason === "invalid_protocol_shape") return localizedCopy(
      language,
      "Required fields are missing or the request shape does not match the selected protocol.",
      "请求字段缺失或结构不符合所选协议。",
      "必要欄位缺失，或請求結構不符合所選協定。",
      "必須フィールドがないか、リクエスト形式が選択したプロトコルに一致しません。",
    );
    return localizedCopy(
      language,
      "The request could not be converted to Token Station's internal format.",
      "请求无法转换为 Token Station 的内部格式。",
      "無法將請求轉換為 Token Station 的內部格式。",
      "リクエストを Token Station の内部形式に変換できませんでした。",
    );
  })();
  return {
    layer: localizedCopy(
      language,
      "Local request conversion",
      "本地请求转换",
      "本機請求轉換",
      "ローカルリクエスト変換",
    ),
    message: `${cause} ${localizedCopy(
      language,
      "The request was not sent upstream.",
      "请求尚未发往上游。",
      "請求尚未傳送至上游。",
      "リクエストはアップストリームに送信されていません。",
    )}`,
    suggestion: reason === "provider_tool_unsupported"
      ? localizedCopy(
          language,
          "Use a native provider that supports the hosted tool, or disable that tool.",
          "改用真正支持该托管工具的原生 Provider，或关闭该工具。",
          "改用支援該託管工具的原生 Provider，或停用該工具。",
          "そのツールをサポートするネイティブ Provider を使用するか、ツールを無効にしてください。",
        )
      : localizedCopy(
          language,
          "Check the Agent protocol and required fields, then retry.",
          "检查 Agent 请求协议和必填字段，然后重试。",
          "檢查 Agent 請求協定和必要欄位，然後重試。",
          "Agent のプロトコルと必須フィールドを確認して再試行してください。",
        ),
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
  zhTW: string;
  ja: string;
}

interface LocalizedAppMessage {
  en: string;
  zh: string;
  zhTW: string;
  ja: string;
}

function modelContractGuidance(error: unknown): LocalizedAppMessage | null {
  if (!error || typeof error !== "object") return null;
  const code = errorField(error, "code");
  const targetValue = errorField(error, "target");
  const safeTarget = sanitizeAppErrorIdentity(targetValue, 256);
  const target = safeTarget
    ? `\`${safeTarget}\``
    : null;
  const subject = target ?? "the selected model";
  const zhSubject = target ?? "当前模型";
  const guidance: Record<string, LocalizedAppMessage> = {
    agent_runtime_transition: {
      en: "The proxy is switching runtime instances. Wait for the transition to finish; OpenCode readiness will then be checked again.",
      zh: "代理正在切换运行实例。请等待切换完成，届时将重新检查 OpenCode 接入条件。",
      zhTW: "代理正在切換執行個體。請等待切換完成，屆時會重新檢查 OpenCode 的接入條件。",
      ja: "プロキシは実行インスタンスを切り替えています。切り替えが完了すると、OpenCode の準備状態が再確認されます。",
    },
    model_contract_exact_routing_unsupported: {
      en: "OpenCode's fixed model is incompatible with exact-model routing. Switch this Agent to tiered, quota-first, or direct routing.",
      zh: "OpenCode 固定模型与精确模型路由不兼容。请将该 Agent 切换为分层、额度优先或简单路由。",
      zhTW: "OpenCode 的固定模型與精確模型路由不相容。請將此 Agent 切換為分層、額度優先或直接路由。",
      ja: "OpenCode の固定モデルは完全一致モデルルーティングと互換性がありません。この Agent を階層、クォータ優先、または直接ルーティングに切り替えてください。",
    },
    model_contract_invalid_route: {
      en: "The OpenCode route is invalid. Repair and save the route configuration before connecting.",
      zh: "OpenCode 路由配置无效。请修复并保存路由配置后再接入。",
      zhTW: "OpenCode 路由設定無效。請修正並儲存路由設定後再連線。",
      ja: "OpenCode のルート設定が無効です。接続する前にルート設定を修正して保存してください。",
    },
    model_contract_no_reachable_model: {
      en: "The current OpenCode route has no reachable model. Add a reachable provider and model, then save the route.",
      zh: "OpenCode 当前路由没有可达模型。请添加可达的供应商和模型，然后保存路由。",
      zhTW: "目前的 OpenCode 路由沒有可用的模型。請新增可連線的 Provider 和模型，然後儲存路由。",
      ja: "現在の OpenCode ルートには到達可能なモデルがありません。利用可能な Provider とモデルを追加して、ルートを保存してください。",
    },
    model_contract_unknown_provider: {
      en: target
        ? `The OpenCode route references unknown provider ${target}. Repair the route or restore that provider.`
        : "The OpenCode route references a provider that is not available. Repair the route or restore that provider.",
      zh: target
        ? `OpenCode 路由引用了未知供应商 ${target}。请修复路由或恢复该供应商。`
        : "OpenCode 路由引用了不可用的供应商。请修复路由或恢复该供应商。",
      zhTW: target
        ? `OpenCode 路由參照了未知的 Provider ${target}。請修正路由或復原該 Provider。`
        : "OpenCode 路由參照了無法使用的 Provider。請修正路由或復原該 Provider。",
      ja: target
        ? `OpenCode ルートは不明な Provider ${target} を参照しています。ルートを修正するか、その Provider を復元してください。`
        : "OpenCode ルートは利用できない Provider を参照しています。ルートを修正するか、その Provider を復元してください。",
    },
    model_contract_unknown_model: {
      en: target
        ? `The OpenCode route references unknown model ${target}. Repair the route or add that model.`
        : "The OpenCode route references a model that is not available. Repair the route or add that model.",
      zh: target
        ? `OpenCode 路由引用了未知模型 ${target}。请修复路由或添加该模型。`
        : "OpenCode 路由引用了不可用的模型。请修复路由或添加该模型。",
      zhTW: target
        ? `OpenCode 路由參照了未知模型 ${target}。請修正路由或新增該模型。`
        : "OpenCode 路由參照了無法使用的模型。請修正路由或新增該模型。",
      ja: target
        ? `OpenCode ルートは不明なモデル ${target} を参照しています。ルートを修正するか、そのモデルを追加してください。`
        : "OpenCode ルートは利用できないモデルを参照しています。ルートを修正するか、そのモデルを追加してください。",
    },
    model_contract_missing_context_window: {
      en: `Model ${subject} has no context-window limit. Complete this model's limits in Providers, then restart the proxy.`,
      zh: `模型 ${zhSubject} 缺少上下文上限。请前往供应商页面完善该模型限制，然后重启代理。`,
      zhTW: `模型 ${zhSubject} 缺少上下文視窗上限。請在 Provider 頁面補齊此模型的限制，然後重新啟動代理。`,
      ja: `モデル ${subject} にコンテキストウィンドウの上限がありません。Provider でモデルの制限を設定し、プロキシを再起動してください。`,
    },
    model_contract_missing_max_output_tokens: {
      en: `Model ${subject} has no maximum output token limit. Complete this model's limits in Providers, then restart the proxy.`,
      zh: `模型 ${zhSubject} 缺少最大输出 Token 上限。请前往供应商页面完善该模型限制，然后重启代理。`,
      zhTW: `模型 ${zhSubject} 缺少最大輸出 Token 上限。請在 Provider 頁面補齊此模型的限制，然後重新啟動代理。`,
      ja: `モデル ${subject} に最大出力トークンの上限がありません。Provider でモデルの制限を設定し、プロキシを再起動してください。`,
    },
    model_contract_invalid_limits: {
      en: `Model ${subject} has invalid token limits. Set maximum output below its context window, then restart the proxy.`,
      zh: `模型 ${zhSubject} 的 Token 上限无效。请将最大输出设为小于上下文上限，然后重启代理。`,
      zhTW: `模型 ${zhSubject} 的 Token 上限無效。請將最大輸出設為小於上下文上限，然後重新啟動代理。`,
      ja: `モデル ${subject} のトークン上限が無効です。最大出力をコンテキストウィンドウ未満に設定し、プロキシを再起動してください。`,
    },
  };
  return guidance[code] ?? null;
}

function routerConfigGuidance(raw: string): LocalizedAppMessage | null {
  const emptyPool = raw.match(/pool `([^`\r\n]{1,128})` has no members/i);
  if (emptyPool) {
    const pool = sanitizeAppErrorIdentity(emptyPool[1]);
    if (!pool) {
      return {
        en: "The selected route pool is empty. Add a provider and model to this pool, then save again.",
        zh: "当前路由池为空。请为该路由池添加供应商和模型，然后重新保存。",
        zhTW: "目前的路由集區是空的。請為此路由集區新增 Provider 和模型，然後重新儲存。",
        ja: "選択したルートプールは空です。このプールに Provider とモデルを追加して、もう一度保存してください。",
      };
    }
    return {
      en: `Route pool \`${pool}\` is empty. Add a provider and model to this pool, then save again.`,
      zh: `路由池 \`${pool}\` 为空。请为该路由池添加供应商和模型，然后重新保存。`,
      zhTW: `路由集區 \`${pool}\` 是空的。請為此路由集區新增 Provider 和模型，然後重新儲存。`,
      ja: `ルートプール \`${pool}\` は空です。このプールに Provider とモデルを追加して、もう一度保存してください。`,
    };
  }

  const unknownPool = raw.match(
    /([^\r\n]{1,128}?) routes to pool `([^`\r\n]{1,128})`, which does not exist/i,
  );
  if (unknownPool) {
    const reference = sanitizeAppErrorIdentity(unknownPool[1]);
    const pool = sanitizeAppErrorIdentity(unknownPool[2]);
    if (!reference || !pool) {
      return {
        en: "A routing setting points to a missing route pool. Choose an existing pool, then save again.",
        zh: "一个路由设置指向不存在的路由池。请选择现有路由池，然后重新保存。",
        zhTW: "一項路由設定指向不存在的路由集區。請選擇現有的路由集區，然後重新儲存。",
        ja: "ルーティング設定が存在しないルートプールを参照しています。既存のプールを選択して、もう一度保存してください。",
      };
    }
    const rule = reference.match(/^rule `([^`\r\n]{1,128})`$/i);
    const englishReference = rule ? `Rule \`${rule[1]}\`` : `Configuration field \`${reference}\``;
    const chineseReference = rule ? `规则 \`${rule[1]}\`` : `配置字段 \`${reference}\``;
    const traditionalChineseReference = rule ? `規則 \`${rule[1]}\`` : `設定欄位 \`${reference}\``;
    const japaneseReference = rule ? `ルール \`${rule[1]}\`` : `設定フィールド \`${reference}\``;
    return {
      en: `${englishReference} points to missing route pool \`${pool}\`. Choose an existing pool for this ${rule ? "rule" : "field"}, then save again.`,
      zh: `${chineseReference} 指向不存在的路由池 \`${pool}\`。请为该${rule ? "规则" : "字段"}选择现有路由池，然后重新保存。`,
      zhTW: `${traditionalChineseReference} 指向不存在的路由集區 \`${pool}\`。請為此${rule ? "規則" : "欄位"}選擇現有的路由集區，然後重新儲存。`,
      ja: `${japaneseReference} は存在しないルートプール \`${pool}\` を参照しています。この${rule ? "ルール" : "フィールド"}に既存のプールを選択して、もう一度保存してください。`,
    };
  }

  return null;
}

const APP_ERROR_GUIDANCE: LocalizedAppError[] = [
  {
    matches: /cursor_running|Cursor (?:is|still) running|Cursor (?:正在|仍在)运行/i,
    en: "Cursor is still running. Quit Cursor completely, then click Connect again. Token Station will not close it for you.",
    zh: "Cursor 仍在运行。请彻底退出 Cursor 后再点一次一键接入。Token Station 不会强制关闭它。",
    zhTW: "Cursor 仍在執行。請完全退出 Cursor，然後再次點選連線。Token Station 不會強制關閉它。",
    ja: "Cursor がまだ実行中です。Cursor を完全に終了してから、もう一度接続してください。Token Station が強制終了することはありません。",
  },
  {
    matches: /cloudflared|Cloudflare Quick Tunnel|Cursor 公网隧道/i,
    en: "Token Station could not establish the Cursor HTTPS tunnel. Check the network, then try again. The temporary endpoint has been closed.",
    zh: "Token Station 无法建立 Cursor HTTPS 隧道。请检查网络后重试。本次临时入口已经关闭。",
    zhTW: "Token Station 無法建立 Cursor HTTPS 通道。請檢查網路後重試。本次暫時端點已關閉。",
    ja: "Token Station は Cursor HTTPS トンネルを確立できませんでした。ネットワークを確認して再試行してください。一時エンドポイントは閉じられました。",
  },
  {
    matches: /apply_in_progress|configuration update.*(?:progress|running)|配置.*(?:正在应用|处理中)/i,
    en: "Another configuration update is still running. Wait a moment, then try again.",
    zh: "另一项配置更新仍在进行。请稍等片刻，然后重试。",
    zhTW: "另一項設定更新仍在進行。請稍候片刻再重試。",
    ja: "別の設定更新がまだ実行中です。しばらく待ってから再試行してください。",
  },
  {
    matches: /quota_(?:accounts_required|account_incomplete)|额度优先.*(?:至少需要|缺少).*(?:供应商|模型)/i,
    en: "A quota account is incomplete. Select both a provider and a model, then apply again.",
    zh: "有一个额度账户尚未配置完整。请同时选择供应商和模型，然后重新应用。",
    zhTW: "有一個額度帳戶尚未設定完整。請同時選擇 Provider 和模型，然後重新套用。",
    ja: "クォータアカウントの設定が不完全です。Provider とモデルの両方を選択して、もう一度適用してください。",
  },
  {
    matches: /a router with no pools can route nothing|没有设置路由池|no route pools?|routing.*not configured/i,
    en: "Routing is not configured yet. Select a provider and model for at least one route, then save before starting Token Station.",
    zh: "路由尚未配置。请至少为一个路由选择供应商和模型，保存后再启动 Token Station。",
    zhTW: "尚未設定路由。請至少為一條路由選擇 Provider 和模型，儲存後再啟動 Token Station。",
    ja: "ルーティングがまだ設定されていません。少なくとも 1 つのルートに Provider とモデルを選択し、保存してから Token Station を起動してください。",
  },
  {
    matches: /路由池.*(?:缺少|未配置)|route pool.*(?:missing|incomplete)|tier.*(?:missing|unconfigured)/i,
    en: "A route is incomplete. Select both a provider and a model for that route, then save again.",
    zh: "有一个路由尚未配置完整。请同时选择供应商和模型，然后重新保存。",
    zhTW: "有一條路由尚未設定完整。請為該路由同時選擇 Provider 和模型，然後重新儲存。",
    ja: "ルートの設定が不完全です。そのルートの Provider とモデルの両方を選択して、もう一度保存してください。",
  },
  {
    matches: /unknown (?:provider|upstream)|未知供应商|未知上游|provider.*not found/i,
    en: "The selected provider is no longer available. Choose an existing provider and save again.",
    zh: "所选供应商已不可用。请选择现有供应商，然后重新保存。",
    zhTW: "所選的 Provider 已無法使用。請選擇現有的 Provider，然後重新儲存。",
    ja: "選択した Provider は利用できなくなりました。既存の Provider を選択して、もう一度保存してください。",
  },
  {
    matches: /unknown model|未知模型|model.*not found|模型.*不存在/i,
    en: "The selected model is no longer available. Refresh the model list and choose another model.",
    zh: "所选模型已不可用。请刷新模型列表，然后选择其他模型。",
    zhTW: "所選模型已無法使用。請重新整理模型清單，然後選擇其他模型。",
    ja: "選択したモデルは利用できなくなりました。モデル一覧を更新して、別のモデルを選択してください。",
  },
  {
    matches: /unknown agent|未知 Agent|agent.*not found|计划不存在或已消费/i,
    en: "This Agent is no longer available or its setup has expired. Rescan Agents and start the setup again.",
    zh: "这个 Agent 已不可用，或接入操作已经过期。请重新扫描 Agent，然后再次开始接入。",
    zhTW: "此 Agent 已無法使用，或設定程序已過期。請重新掃描 Agent，然後再次開始設定。",
    ja: "この Agent は利用できないか、設定の有効期限が切れています。Agent を再スキャンして、設定をやり直してください。",
  },
  {
    matches: /缺少供应商和模型|(?:high|mid|low|上|中|下).*档.*(?:缺少|未配置)/i,
    en: "A route is incomplete. Select both a provider and a model for that route, then save again.",
    zh: "有一个路由尚未配置完整。请同时选择供应商和模型，然后重新保存。",
    zhTW: "有一條路由尚未設定完整。請為該路由同時選擇 Provider 和模型，然後重新儲存。",
    ja: "ルートの設定が不完全です。そのルートの Provider とモデルの両方を選択して、もう一度保存してください。",
  },
  {
    matches: /(?:\b(?:invalid|expired|rejected|denied|missing|required)\b.{0,48}\b(?:credential|api[ _-]?key|auth(?:entication|orization)?)\b|\b(?:credential|api[ _-]?key|auth(?:entication|orization)?)\b.{0,48}\b(?:invalid|expired|rejected|denied|missing|required)\b|鉴权失败|凭据(?:无效|被拒绝|缺失|不可用)|密钥(?:无效|被拒绝|缺失|不可用))/i,
    en: "The credential could not be used. Check the API key and its permissions, then try again.",
    zh: "凭据无法使用。请检查 API Key 及其权限，然后重试。",
    zhTW: "無法使用此憑證。請檢查 API Key 及其權限，然後重試。",
    ja: "認証情報を使用できませんでした。API Key とその権限を確認して、再試行してください。",
  },
  {
    matches: /open_external_failed|could not open.*(?:page|url)|无法打开外部页面/i,
    en: "The external page could not be opened. Check the URL and your default browser, then try again.",
    zh: "无法打开外部页面。请检查网址和默认浏览器，然后重试。",
    zhTW: "無法開啟外部頁面。請檢查 URL 和預設瀏覽器，然後重試。",
    ja: "外部ページを開けませんでした。URL と既定のブラウザを確認して、再試行してください。",
  },
  {
    matches: /Agent .*model metadata refresh failed|Agent 模型元数据刷新失败/i,
    en: "The proxy is running, but Token Station could not refresh one managed Agent configuration. Open the Agent page, rescan, and repair that Agent before using its route.",
    zh: "代理已经运行，但一个已接管 Agent 的模型元数据刷新失败。请打开 Agent 页面重新扫描，并修复该 Agent 后再使用它的路由。",
    zhTW: "代理正在執行，但 Token Station 無法重新整理其中一個受管理 Agent 的設定。請開啟 Agent 頁面重新掃描並修復該 Agent，再使用其路由。",
    ja: "プロキシは実行中ですが、Token Station は管理対象 Agent の設定を更新できませんでした。Agent ページで再スキャンし、その Agent を修復してからルートを使用してください。",
  },
  {
    matches: /Kimi Code.*(?:positive|正数).*(?:context|max_context_size)|Kimi Code.*(?:context|max_context_size).*(?:missing|required|需要|缺少)/i,
    en: "Kimi Code needs a verified context-window limit for the active route. Complete that model's context window in Providers, restart the proxy, then connect again.",
    zh: "Kimi Code 需要当前路由模型具备可信的上下文上限。请在供应商页面补全该模型的上下文窗口，重启代理后再次接入。",
    zhTW: "Kimi Code 需要目前路由模型具有已驗證的上下文視窗上限。請在 Provider 頁面補齊該模型的上下文視窗，重新啟動代理後再連線。",
    ja: "Kimi Code には、現在のルートで検証済みのコンテキストウィンドウ上限が必要です。Provider でモデルの上限を設定し、プロキシを再起動してから再接続してください。",
  },
  {
    matches: /\bVERSION_PROBE_TIMEOUT\b/i,
    en: "Agent version detection timed out. Rescan; if it still fails, check that the Agent installation is complete.",
    zh: "Agent 版本检测超时。请重新扫描；如果仍然失败，请检查该 Agent 的安装是否完整。",
    zhTW: "Agent 版本偵測逾時。請重新掃描；若仍失敗，請檢查該 Agent 是否已完整安裝。",
    ja: "Agent のバージョン検出がタイムアウトしました。再スキャンし、それでも失敗する場合は Agent が完全にインストールされているか確認してください。",
  },
  {
    matches: /(?:(?:network|request|upstream|connect(?:ion)?|dns|tls).{0,80}(?:timed? ?out|timeout)|(?:timed? ?out|timeout).{0,80}(?:network|request|upstream|connect(?:ion)?|dns|tls)|(?:网络|请求|上游|连接).{0,40}超时|超时.{0,40}(?:网络|请求|上游|连接))/i,
    en: "The operation took too long and was stopped. Check the network connection, then try again.",
    zh: "操作等待时间过长，已停止。请检查网络连接，然后重试。",
    zhTW: "操作耗時過長，已停止。請檢查網路連線，然後重試。",
    ja: "処理に時間がかかりすぎたため停止しました。ネットワーク接続を確認して、再試行してください。",
  },
  {
    matches: /network|dns|tls|certificate|request failed|connection (?:refused|reset|closed|failed)|connect(?:ion)? error|网络|连接失败|证书/i,
    en: "Token Station could not reach the service. Check the network, Base URL, and proxy settings, then try again.",
    zh: "Token Station 无法连接到服务。请检查网络、Base URL 和代理设置，然后重试。",
    zhTW: "Token Station 無法連線到服務。請檢查網路、Base URL 和代理設定，然後重試。",
    ja: "Token Station はサービスに接続できませんでした。ネットワーク、Base URL、プロキシ設定を確認して、再試行してください。",
  },
  {
    matches: /invalid.*(?:url|address)|proxy address|代理地址无效|地址.*不合法/i,
    en: "The address is not valid. Enter a complete HTTP, HTTPS, or SOCKS5 address and try again.",
    zh: "地址格式不正确。请输入完整的 HTTP、HTTPS 或 SOCKS5 地址，然后重试。",
    zhTW: "位址格式無效。請輸入完整的 HTTP、HTTPS 或 SOCKS5 位址，然後重試。",
    ja: "アドレスが無効です。完全な HTTP、HTTPS、または SOCKS5 アドレスを入力して、再試行してください。",
  },
  {
    matches: /address already in use|port.*(?:used|busy)|listener.*(?:bind|address|port)|listen_restore|端口.*占用|监听地址/i,
    en: "The local proxy could not use its configured address. Close the app using that port or choose another port, then restart the proxy.",
    zh: "本地代理无法使用当前监听地址。请关闭占用该端口的应用，或换一个端口，然后重启代理。",
    zhTW: "本機代理無法使用目前設定的位址。請關閉占用該連接埠的應用程式，或選擇其他連接埠，然後重新啟動代理。",
    ja: "ローカルプロキシは設定されたアドレスを使用できませんでした。そのポートを使用しているアプリを終了するか別のポートを選び、プロキシを再起動してください。",
  },
  {
    matches: /proxy.*not running|代理未运行/i,
    en: "The local proxy is not running. Start the proxy, then try again.",
    zh: "本地代理尚未运行。请先启动代理，然后重试。",
    zhTW: "本機代理尚未執行。請先啟動代理，然後重試。",
    ja: "ローカルプロキシが実行されていません。プロキシを起動してから再試行してください。",
  },
  {
    matches: /proxy.*(?:start|restart)|代理启动|启动代理|ensure_serve_running|gateway_(?:init|restore)|listen_(?:publish|nonblocking)/i,
    en: "The local proxy could not restart safely. Quit and reopen Token Station, then try again.",
    zh: "本地代理无法安全重启。请退出并重新打开 Token Station，然后重试。",
    zhTW: "本機代理無法安全地重新啟動。請結束並重新開啟 Token Station，然後重試。",
    ja: "ローカルプロキシを安全に再起動できませんでした。Token Station を終了して開き直し、再試行してください。",
  },
  {
    matches: /state_poisoned|app_state_poisoned|写锁已损坏|状态不可用|operation_.*poisoned/i,
    en: "Token Station's local state is temporarily unavailable. Restart the app; your saved configuration will remain unchanged.",
    zh: "Token Station 的本地状态暂时不可用。请重启应用，已保存的配置不会改变。",
    zhTW: "Token Station 的本機狀態暫時無法使用。請重新啟動應用程式；已儲存的設定不會變更。",
    ja: "Token Station のローカル状態は一時的に利用できません。アプリを再起動してください。保存済みの設定は変更されません。",
  },
  {
    matches: /resource limit|资源上限|response.*(?:too large|MiB)|超过.*限制/i,
    en: "The service returned more data than Token Station can safely process. Narrow the request or try again later.",
    zh: "服务返回的数据超过 Token Station 的安全处理上限。请缩小请求范围，或稍后重试。",
    zhTW: "服務傳回的資料超過 Token Station 可安全處理的上限。請縮小請求範圍或稍後重試。",
    ja: "サービスから Token Station が安全に処理できる量を超えるデータが返されました。リクエストを絞るか、後でもう一度お試しください。",
  },
  {
    matches: /model_providers|configuration (?:format|file.*(?:format|invalid|parse|syntax))|config(?:uration)? file.*(?:format|invalid|parse|syntax)|配置(?:文件)?(?:格式|结构)|JSON5|TOML|YAML/i,
    en: "The configuration file has a format Token Station cannot safely edit. Fix the file syntax or restore a known-good backup, then rescan.",
    zh: "配置文件格式无法安全编辑。请修复文件语法，或恢复可用备份，然后重新扫描。",
    zhTW: "設定檔的格式無法由 Token Station 安全編輯。請修正檔案語法或復原已知可用的備份，然後重新掃描。",
    ja: "設定ファイルは Token Station が安全に編集できない形式です。構文を修正するか正常なバックアップを復元して、再スキャンしてください。",
  },
  {
    matches: /baseline|before_hash|revision_hash|前置值|基线快照|revision.*changed|配置.*已变化/i,
    en: "The configuration changed after it was opened. Reload the latest version, review it, and apply the change again.",
    zh: "配置在打开后又发生了变化。请重新加载最新版本，确认后再次应用。",
    zhTW: "設定在開啟後已發生變更。請重新載入最新版本、確認內容，然後再次套用變更。",
    ja: "開いた後に設定が変更されました。最新版を再読み込みして確認し、もう一度変更を適用してください。",
  },
  {
    matches: /Key 无效，或当前账号没有读取模型目录的权限|model catalog.*(?:401|403)|模型目录.*(?:401|403)/i,
    en: "The API key is invalid, or this account cannot read the provider's model catalog.",
    zh: "API Key 无效，或当前账号没有读取该供应商模型目录的权限。",
    zhTW: "API Key 無效，或目前帳戶沒有讀取該 Provider 模型目錄的權限。",
    ja: "API Key が無効か、このアカウントには Provider のモデルカタログを読み取る権限がありません。",
  },
  {
    matches: /(?:(?:file|directory|path|disk|filesystem).*(?:permission denied|read-only)|(?:permission denied|read-only file).*(?:file|directory|path)|(?:文件|目录|路径|磁盘).*(?:权限|只读))/i,
    en: "Token Station could not access the required local file. Check file permissions and available disk space, then try again.",
    zh: "Token Station 无法访问所需的本地文件。请检查文件权限和磁盘空间，然后重试。",
    zhTW: "Token Station 無法存取所需的本機檔案。請檢查檔案權限和可用磁碟空間，然後重試。",
    ja: "Token Station は必要なローカルファイルにアクセスできませんでした。ファイル権限と空きディスク容量を確認して、再試行してください。",
  },
  {
    matches: /database|sqlite|metrics?.*schema|schema.*metrics?|指标库|数据库/i,
    en: "The local data could not be opened. Update Token Station and try again; if the problem continues, contact support.",
    zh: "无法打开本地数据。请更新 Token Station 后重试；如果仍然失败，请联系支持。",
    zhTW: "無法開啟本機資料。請更新 Token Station 後重試；如果仍然失敗，請聯絡支援。",
    ja: "ローカルデータを開けません。Token Station を更新して再試行し、問題が続く場合はサポートにお問い合わせください。",
  },
  {
    matches: /model_catalog_provider_required/i,
    en: "Enter a provider name and Base URL before loading its model catalog.",
    zh: "请先填写供应商名称和 Base URL，再读取模型目录。",
    zhTW: "請先輸入 Provider 名稱和 Base URL，再載入模型目錄。",
    ja: "モデルカタログを読み込む前に、Provider 名と Base URL を入力してください。",
  },
  {
    matches: /model_catalog_reference_requires_save/i,
    en: "Save the environment-variable or file credential reference before loading the model catalog.",
    zh: "env/file 只保存凭据引用。请先保存供应商，再读取模型目录。",
    zhTW: "env/file 只會儲存憑證參照。請先儲存 Provider，再載入模型目錄。",
    ja: "環境変数またはファイルの認証情報参照を保存してから、モデルカタログを読み込んでください。",
  },
  {
    matches: /model_catalog_api_key_required/i,
    en: "Enter an API key before loading this provider's model catalog.",
    zh: "请先填写 API Key，再读取该供应商的模型目录。",
    zhTW: "請先輸入 API Key，再載入此 Provider 的模型目錄。",
    ja: "この Provider のモデルカタログを読み込む前に、API Key を入力してください。",
  },
  {
    matches: /model_catalog_azure_deployment_manual/i,
    en: "Enter the Azure deployment name manually; this dialect does not use the generic model-catalog request.",
    zh: "Azure deployment name 需要手工填写；该方言不使用通用模型目录请求。",
    zhTW: "請手動輸入 Azure 部署名稱；此方言不使用通用模型目錄請求。",
    ja: "Azure のデプロイ名を手動で入力してください。この方言では汎用モデルカタログ要求を使用しません。",
  },
  {
    matches: /(?:(?:quota|usage|pricing|catalog).{0,80}(?:fetch|refresh|load|read|unavailable|failed|error|down)|(?:fetch|refresh|load|read).{0,80}(?:quota|usage|pricing|catalog)|(?:额度|用量|价格目录|模型目录).{0,40}(?:查询|刷新|读取|获取|失败|不可用)|(?:查询|刷新|读取|获取).{0,40}(?:额度|用量|价格目录|模型目录))/i,
    en: "The latest provider data is unavailable. Keep the current settings and try refreshing again later.",
    zh: "暂时无法获取最新的供应商数据。请保留当前设置，稍后再次刷新。",
    zhTW: "目前無法取得最新的 Provider 資料。請保留目前設定，稍後再重新整理。",
    ja: "最新の Provider データを取得できません。現在の設定を維持し、後でもう一度更新してください。",
  },
  {
    matches: /failed to (?:read|write) (?:file|directory|config|cache|database|local)|无法(?:读取|写入).*(?:文件|目录|配置|缓存|数据库|本地)/i,
    en: "Token Station could not access the required local file. Check file permissions and available disk space, then try again.",
    zh: "Token Station 无法访问所需的本地文件。请检查文件权限和磁盘空间，然后重试。",
    zhTW: "Token Station 無法存取所需的本機檔案。請檢查檔案權限和可用磁碟空間，然後重試。",
    ja: "Token Station は必要なローカルファイルにアクセスできませんでした。ファイル権限と空きディスク容量を確認して、再試行してください。",
  },
  {
    matches: /暂无公开发布版本|no public release (?:is )?available/i,
    en: "No public release is available yet. Check again later or open the Releases page.",
    zh: "暂无公开发布版本。请稍后重试，或打开发布页查看。",
    zhTW: "目前沒有公開發行版本。請稍後再試，或開啟 Releases 頁面查看。",
    ja: "公開リリースはまだありません。後でもう一度確認するか、Releases ページを開いてください。",
  },
  {
    matches: /官方更新公钥|official update public key/i,
    en: "This build cannot install updates because it does not include the official update public key. Download the app from the Releases page instead.",
    zh: "当前构建未内置官方更新公钥，无法在 App 内安装更新。请改从发布页下载安装。",
    zhTW: "此版本未包含官方更新公開金鑰，因此無法安裝更新。請改從 Releases 頁面下載應用程式。",
    ja: "このビルドには公式の更新公開鍵が含まれていないため、更新をインストールできません。代わりに Releases ページからアプリをダウンロードしてください。",
  },
  {
    matches: /update_version_changed:/i,
    en: "A newer update became available. Check again before installing.",
    zh: "可用更新已经发生变化。请重新检查后再确认安装。",
    zhTW: "已有較新的更新可用。請在安裝前重新檢查。",
    ja: "新しい更新が利用可能になりました。インストール前にもう一度確認してください。",
  },
  {
    matches: /update_expected_version_missing:/i,
    en: "The selected update is no longer available. Check again before installing.",
    zh: "之前确认的更新已不可用。请重新检查后再确认安装。",
    zhTW: "所選的更新已無法使用。請在安裝前重新檢查。",
    ja: "選択した更新は利用できなくなりました。インストール前にもう一度確認してください。",
  },
  {
    matches: /update_(?:in_progress|gateway|manifest|expected|version|download|install)|(?:check|install|apply|download).{0,40}(?:update|upgrade)|(?:update|upgrade).{0,40}(?:failed|unavailable|error|down)|检查更新|安装更新|应用更新|可用版本|发布版本/i,
    en: "Token Station could not check for updates. Check the network connection and try again later.",
    zh: "Token Station 无法检查更新。请检查网络连接，稍后重试。",
    zhTW: "Token Station 無法檢查更新。請檢查網路連線，稍後再試。",
    ja: "Token Station は更新を確認できませんでした。ネットワーク接続を確認して、後でもう一度お試しください。",
  },
];

const MAX_APP_ERROR_DETAIL_LENGTH = 320;
const MAX_APP_ERROR_MATCH_LENGTH = 4_096;

function errorField(error: object, field: string): string {
  try {
    const value = (error as Record<string, unknown>)[field];
    return typeof value === "string" || typeof value === "number" ? String(value) : "";
  } catch {
    return "";
  }
}

function nativeErrorMessage(error: unknown): string | null {
  try {
    if (!(error instanceof Error)) return null;
    return typeof error.message === "string" ? error.message : "";
  } catch {
    return null;
  }
}

function boundedAppErrorText(value: string): string {
  return value.slice(0, MAX_APP_ERROR_MATCH_LENGTH);
}

function appErrorText(error: unknown): string {
  if (typeof error === "string") return boundedAppErrorText(error);
  const nativeMessage = nativeErrorMessage(error);
  if (nativeMessage !== null) return boundedAppErrorText(nativeMessage);
  if (error && typeof error === "object") {
    return boundedAppErrorText(["code", "reason_code", "field", "message", "suggestion", "detail"]
      .map((field) => errorField(error, field))
      .filter(Boolean)
      .join(" "));
  }
  return boundedAppErrorText(String(error ?? ""));
}

function appErrorDetail(error: unknown): string {
  if (typeof error === "string") return error;
  const nativeMessage = nativeErrorMessage(error);
  if (nativeMessage !== null) return nativeMessage;
  if (!error || typeof error !== "object") return "";
  return ["message", "suggestion", "detail"]
    .map((field) => errorField(error, field))
    .find(Boolean) ?? "";
}

function containsPrivateDiagnostic(raw: string): boolean {
  return /(?:^|\n)\s*at\s+\S/m.test(raw)
    || /\b(?:stack backtrace|stack trace|panicked at|panic:|fatal runtime error)\b/i.test(raw)
    || /\b(?:secret\s+)?internal\s+(?:detail|diagnostic|transaction detail|implementation detail)\b/i.test(raw)
    || /\bSQLSTATE\b/i.test(raw)
    || /\b(?:SELECT\s+.+\s+FROM|INSERT\s+INTO|UPDATE\s+.+\s+SET|DELETE\s+FROM)\b/is.test(raw);
}

function sanitizeAppErrorDetail(error: unknown): string | null {
  const raw = boundedAppErrorText(appErrorDetail(error)).trim();
  if (!raw || containsPrivateDiagnostic(raw)) return null;

  let detail = raw
    .replace(/(\bauthorization\s+bearer\s+)[^\s,;]{8,}/gi, "$1[redacted]")
    .replace(/(\bauthorization\s*[:=：]\s*)(?:bearer\s+)?[^\s,;]+/gi, "$1[redacted]")
    .replace(/([?&](?:api[_-]?key|access[_-]?token|token|password|secret|key)=)[^&\s]+/gi, "$1[redacted]")
    .replace(/([a-z][a-z0-9+.-]*:\/\/)[^/\s:@]+:[^/\s@]+@/gi, "$1[redacted]@")
    .replace(/("(?:api[_ -]?key|access[_ -]?token|token|password|secret|credential|private[_ -]?key|令牌|密码|口令|凭据|密钥)"\s*[:=：]\s*)(?:"[^"]*"|'[^']*'|“[^”]*”|‘[^’]*’|[^\s,;，；}]+)/gi, '$1"[redacted]"')
    .replace(/((?:\b(?:api[_ -]?key|access[_ -]?token|token|password|secret|credential|private[_ -]?key)\b|(?:令牌|密码|口令|凭据|密钥))\s*[:=：]\s*)(?:"[^"]*"|'[^']*'|“[^”]*”|‘[^’]*’|[^\s,;，；}]+)/gi, "$1[redacted]")
    .replace(/\b(?:sk-[A-Za-z0-9_-]{12,}|AIza[A-Za-z0-9_-]{20,}|AKIA[A-Z0-9]{16}|gh[pousr]_[A-Za-z0-9_]{20,}|xox[baprs]-[A-Za-z0-9-]{12,}|eyJ[A-Za-z0-9_-]{16,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,})\b/g, "[redacted]")
    .replace(/(?:~\/|\/(?:Users|home|private|var|tmp|etc|opt|Library)\/)[^:：\r\n，；。！？]*?\.(?:json5?|toml|ya?ml|sqlite3?|db|log|txt|tsx?|jsx?|mjs|cjs|rs|wasm|lock)(?=[:：,，;；\s]|$)/gi, "[local path]")
    .replace(/[A-Za-z]:\\[^:：\r\n，；。！？]*?\.(?:json5?|toml|ya?ml|sqlite3?|db|log|txt|tsx?|jsx?|mjs|cjs|rs|wasm|lock)(?=[:：,，;；\s]|$)/gi, "[local path]")
    .replace(/(?:~\/|\/(?:Users|home|private|var|tmp|etc|opt|Library)\/)[^\s,;)"'`]+/g, "[local path]")
    .replace(/[A-Za-z]:\\[^\s,;)"'`]+/g, "[local path]")
    .replace(/[\u0000-\u001f\u007f]+/g, " ")
    .replace(/\s+/g, " ")
    .trim();

  if (!detail || /^\[?(?:redacted|local path)\]?$/i.test(detail)) return null;
  if (detail.length > MAX_APP_ERROR_DETAIL_LENGTH) {
    detail = `${detail.slice(0, MAX_APP_ERROR_DETAIL_LENGTH - 1).trimEnd()}…`;
  }
  return detail;
}

function sanitizeAppErrorIdentity(value: string, maxLength = 128): string | null {
  const trimmed = value.trim();
  if (!trimmed || trimmed.length > maxLength) return null;
  const sanitized = sanitizeAppErrorDetail(trimmed);
  return sanitized === trimmed ? sanitized : null;
}

function selectedAppLanguage(language?: Language): Language {
  if (language) return language;
  const active = getActiveLanguage();
  if (active) return active;
  try {
    const stored = window.localStorage.getItem("token-station-language");
    return stored === "zh-CN" || stored === "zh-TW" || stored === "ja" ? stored : "en";
  } catch {
    return "en";
  }
}

function genericAppError(language: Language): string {
  return localizedCopy(
    language,
    "The operation could not be completed. Try again. If it still fails, update Token Station or contact support.",
    "操作未能完成。请重试；如果仍然失败，请更新 Token Station 或联系支持。",
    "操作未能完成。請重試。如果仍然失敗，請更新 Token Station 或聯絡支援。",
    "操作を完了できませんでした。もう一度お試しください。問題が続く場合は Token Station を更新するかサポートにお問い合わせください。",
  );
}

function detailedAppError(detail: string, language: Language): string {
  return localizedCopy(
    language,
    `Operation failed: ${detail}`,
    `操作失败：${detail}`,
    `操作失敗：${detail}`,
    `操作に失敗しました：${detail}`,
  );
}

function appGuidanceForLanguage(
  guidance: LocalizedAppMessage | LocalizedAppError,
  language: Language,
): string {
  return {
    en: guidance.en,
    "zh-CN": guidance.zh,
    "zh-TW": guidance.zhTW,
    ja: guidance.ja,
  }[language];
}

export function humanizeAppError(error: unknown, language?: Language): string {
  const raw = appErrorText(error);
  const selectedLanguage = selectedAppLanguage(language);
  const modelGuidance = modelContractGuidance(error);
  const routerGuidance = routerConfigGuidance(raw);
  const guidance = APP_ERROR_GUIDANCE.find((item) => item.matches.test(raw));
  if (modelGuidance) return appGuidanceForLanguage(modelGuidance, selectedLanguage);
  if (routerGuidance) return appGuidanceForLanguage(routerGuidance, selectedLanguage);
  if (guidance) return appGuidanceForLanguage(guidance, selectedLanguage);
  const detail = sanitizeAppErrorDetail(error);
  if (detail) return detailedAppError(detail, selectedLanguage);
  return genericAppError(selectedLanguage);
}
