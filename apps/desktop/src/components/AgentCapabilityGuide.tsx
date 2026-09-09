import type { AgentRouteView } from "../api";
import { useLocalizedCopy, type LocalizedCopy } from "./LanguageProvider";
import "./AgentCapabilityGuide.css";

function searchGuidance(agentId: string, copy: LocalizedCopy): string | undefined {
  switch (agentId) {
    case "opencode":
      return copy(
        "OpenCode can use Exa search. For a compatible CLI, enable it only for the session with the command below. Check OpenCode tool permissions if search is unavailable. This does not configure the desktop App.",
        "OpenCode 可使用 Exa 搜索。支持此开关的 CLI 可用下方命令仅为本次会话启用；若仍不可用，请检查 OpenCode 的工具权限。此命令不会配置桌面 App。",
        "OpenCode 可使用 Exa 搜尋。支援此開關的 CLI 可用下方命令僅為本次工作階段啟用；若仍無法使用，請檢查 OpenCode 的工具權限。此命令不會設定桌面 App。",
        "OpenCode は Exa 検索を利用できます。対応する CLI では、次のコマンドで今回のセッションだけ有効にできます。利用できない場合はツール権限を確認してください。デスクトップ App の設定は変更されません。",
      );
    case "deepseek-harness":
      return copy(
        "DeepSeek Harness search requires its separate DEEPSEEK_API_KEY credential and search service access. The Token Station model key does not replace this credential. Configure it in DeepSeek Harness, then test search in a new session.",
        "DeepSeek Harness 搜索需要单独的 DEEPSEEK_API_KEY 凭据及搜索服务权限，Token Station 的模型 Key 不能替代它。请在 DeepSeek Harness 中配置，再新建会话测试搜索。",
        "DeepSeek Harness 搜尋需要獨立的 DEEPSEEK_API_KEY 憑證及搜尋服務權限，Token Station 的模型 Key 無法取代它。請在 DeepSeek Harness 中設定，再建立新工作階段測試搜尋。",
        "DeepSeek Harness 検索には専用の DEEPSEEK_API_KEY と検索サービスへのアクセスが必要です。Token Station のモデルキーでは代用できません。DeepSeek Harness で設定し、新しいセッションで検索を確認してください。",
      );
    case "openclaw":
      return copy(
        "Configure and enable a web search provider in OpenClaw. Add that provider's credential if required. A working model connection or web_fetch does not confirm search access.",
        "请在 OpenClaw 中配置并启用网络搜索提供商，并按该服务要求添加凭据。模型连接或 web_fetch 成功，不代表搜索已经可用。",
        "請在 OpenClaw 中設定並啟用網路搜尋提供商，並依該服務要求加入憑證。模型連線或 web_fetch 成功，不代表搜尋已可用。",
        "OpenClaw で Web 検索プロバイダーを設定して有効にしてください。必要に応じて専用の認証情報を追加します。モデル接続や web_fetch の成功だけでは検索の利用可否は確認できません。",
      );
    case "nous-hermes-agent":
      return copy(
        "Configure a supported web search service in Hermes and enable its web tools. Missing service credentials can hide these tools. Configure the service in Hermes, then start a new session.",
        "请在 Hermes 中配置受支持的搜索服务并启用 web 工具；缺少服务凭据时，工具可能不会出现。完成 Hermes 内的配置后，新建会话验证。",
        "請在 Hermes 中設定支援的搜尋服務並啟用 web 工具；缺少服務憑證時，工具可能不會出現。完成 Hermes 內的設定後，建立新工作階段驗證。",
        "Hermes で対応する検索サービスを設定し、web ツールを有効にしてください。認証情報がない場合、ツールが表示されないことがあります。設定後に新しいセッションで確認してください。",
      );
    case "claude-code":
    case "claude-desktop":
      return copy(
        "WebSearch requires a route with native search support. A translated model route alone cannot provide it. Use a compatible search route or configure a search MCP in the client. Claude Desktop webpage access also depends on its egress allowlist.",
        "WebSearch 需要支持原生搜索的路由，仅接通模型转换路由无法提供这项能力。请使用兼容的搜索路由，或在客户端单独配置搜索 MCP。Claude Desktop 读取网页还受出口白名单限制。",
        "WebSearch 需要支援原生搜尋的路由，僅連通模型轉換路由無法提供此能力。請使用相容的搜尋路由，或在客戶端獨立設定搜尋 MCP。Claude Desktop 讀取網頁也受出口白名單限制。",
        "WebSearch にはネイティブ検索対応のルートが必要です。モデル変換ルートへの接続だけでは利用できません。対応ルートを使用するか、クライアントに検索 MCP を設定してください。Claude Desktop のページ取得には送信先許可リストも適用されます。",
      );
    case "gemini-cli":
      return copy(
        "Google Search and URL Context require Google's hosted tools. The current Gemini conversion adapter supports function tools, but not these hosted tools. Configure a separate search MCP for a translated model route.",
        "Google Search 与 URL Context 依赖 Google 托管工具。当前 Gemini 转换适配器支持函数工具，但不支持这些托管工具。使用模型转换路由时，请单独配置搜索 MCP。",
        "Google Search 與 URL Context 依賴 Google 託管工具。目前 Gemini 轉換配接器支援函式工具，但不支援這些託管工具。使用模型轉換路由時，請獨立設定搜尋 MCP。",
        "Google Search と URL Context は Google のホスト型ツールに依存します。現在の Gemini 変換アダプターは関数ツールに対応しますが、これらのホスト型ツールには対応しません。変換ルートでは検索 MCP を別途設定してください。",
      );
    case "kimi-code":
      return copy(
        "Kimi Code FetchURL reads a known URL. It does not provide search. If no search tool is available, configure a supported search service or MCP in Kimi Code.",
        "Kimi Code 的 FetchURL 用于读取已知网址，不等于搜索。若会话没有搜索工具，请在 Kimi Code 中配置受支持的搜索服务或 MCP。",
        "Kimi Code 的 FetchURL 用於讀取已知網址，不等於搜尋。若工作階段沒有搜尋工具，請在 Kimi Code 中設定支援的搜尋服務或 MCP。",
        "Kimi Code の FetchURL は既知の URL を取得する機能です。検索ではありません。検索ツールがない場合は、Kimi Code に対応する検索サービスまたは MCP を設定してください。",
      );
    case "grok-build":
      return copy(
        "Grok Build search availability depends on its backend and session tools. A compatible chat endpoint does not add search tools. Use a search-capable backend or configure a search MCP.",
        "Grok Build 是否提供搜索取决于后端和会话工具；兼容聊天接口不会自动增加搜索工具。请使用具备搜索能力的后端，或单独配置搜索 MCP。",
        "Grok Build 是否提供搜尋取決於後端與工作階段工具；相容聊天介面不會自動加入搜尋工具。請使用具備搜尋能力的後端，或獨立設定搜尋 MCP。",
        "Grok Build の検索はバックエンドとセッションのツールに依存します。互換チャット API だけでは検索ツールは追加されません。検索対応バックエンドまたは検索 MCP を利用してください。",
      );
    case "workbuddy":
      return copy(
        "WorkBuddy provides its own search service. Model access and search access are separate. If search reports an authentication error, check the WorkBuddy session and service access, then retry.",
        "WorkBuddy 使用客户端自身的搜索服务，模型访问与搜索访问相互独立。若搜索出现鉴权错误，请检查 WorkBuddy 会话和服务权限后重试。",
        "WorkBuddy 使用客戶端本身的搜尋服務，模型存取與搜尋存取相互獨立。若搜尋出現驗證錯誤，請檢查 WorkBuddy 工作階段和服務權限後重試。",
        "WorkBuddy は独自の検索サービスを使用します。モデルと検索のアクセス権は別です。検索で認証エラーが出た場合は WorkBuddy のセッションとサービス権限を確認して再試行してください。",
      );
    default:
      return undefined;
  }
}

export default function AgentCapabilityGuide({ agentId, route }: { agentId: string; route: AgentRouteView }) {
  const { copy } = useLocalizedCopy();
  const guidance = searchGuidance(agentId, copy);
  const source = route.inherits_global === true
    ? copy("Follows global", "跟随全局", "跟隨全域", "グローバルに従う")
    : copy("Independent routing", "独立路由", "獨立路由", "独立ルーティング");
  const target = route.routing_mode === "direct"
    ? route.direct_target?.upstream && route.direct_target?.model
      ? `${route.direct_target.upstream} / ${route.direct_target.model}`
      : copy("Select a provider and model", "尚未选择供应商和模型", "尚未選擇供應商和模型", "プロバイダーとモデルを選択してください")
    : route.routing_mode === "quota_first"
      ? copy("Selected by quota policy", "按额度策略选择", "依額度策略選擇", "クォータポリシーで選択")
      : copy("Selected by three-tier routing", "按三档路由选择", "依三檔路由選擇", "三段階ルーティングで選択");

  return (
    <section className="agent-capability-guide agent-flat-surface" aria-label={copy("Routing and search requirements", "路由与搜索说明", "路由與搜尋說明", "ルーティングと検索の説明")}>
      <dl className="agent-capability-route">
        <div><dt>{copy("Routing source", "路由来源", "路由來源", "ルーティングソース")}</dt><dd>{source}</dd></div>
        <div><dt>{copy("Configured target", "配置目标", "設定目標", "設定対象")}</dt><dd>{target}</dd></div>
      </dl>
      {guidance && <details className="agent-search-guidance">
        <summary tabIndex={0}>{copy("Web search setup", "网络搜索配置", "網路搜尋設定", "Web 検索の設定")}</summary>
        <p>{guidance}</p>
        {agentId === "opencode" && <div className="agent-search-commands">
          <span>macOS / Linux</span><code>OPENCODE_ENABLE_EXA=1 opencode</code>
          <span>Windows (cmd.exe)</span><code>cmd /c "set OPENCODE_ENABLE_EXA=1&amp;&amp;opencode"</code>
        </div>}
        <p className="agent-search-disclosure">{copy(
          "Setup guidance only. Search credentials and availability have not been checked here. Fetching a known URL is a separate capability.",
          "这里只提供配置指引，尚未检测搜索凭据和可用性。读取已知网址是另一项能力。",
          "此處僅提供設定指引，尚未檢測搜尋憑證和可用性。讀取已知網址是另一項能力。",
          "ここでは設定方法のみを案内します。検索の認証情報と利用可否は未確認です。既知の URL の取得は別の機能です。",
        )}</p>
      </details>}
    </section>
  );
}
