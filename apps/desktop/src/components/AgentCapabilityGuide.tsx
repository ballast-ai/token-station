import type { AgentId } from "../api";
import { useLocalizedCopy } from "./LanguageProvider";

export default function AgentCapabilityGuide({ agentId }: { agentId: AgentId }) {
  const { copy } = useLocalizedCopy();
  const search: Record<string, string> = {
    "kimi-code": copy(
      "Check whether the selected Kimi provider exposes WebSearch. Chat access alone does not enable this tool.",
      "确认 Kimi 当前 provider 是否提供 WebSearch。模型聊天接入不会自动启用该工具。",
      "確認 Kimi 目前 provider 是否提供 WebSearch。模型聊天連線不會自動啟用該工具。",
      "Kimi の provider が WebSearch を提供するか確認してください。チャット接続だけでは有効になりません。"),
    openclaw: copy(
      "Select the search provider in OpenClaw and check its credentials. Automatic selection can use an inherited environment key.",
      "在 OpenClaw 中明确选择搜索服务并检查凭证。自动选择可能使用环境中已有的 Key。",
      "在 OpenClaw 中明確選擇搜尋服務並檢查憑證。自動選擇可能使用環境中已有的 Key。",
      "OpenClaw で検索サービスと認証情報を確認してください。自動選択では既存の環境変数が使われる場合があります。"),
    workbuddy: copy(
      "Confirm the official account is signed in inside WorkBuddy, then run native search. A model API key is not a search login.",
      "在 WorkBuddy 内确认官方账号已登录，再运行原生搜索。模型 API Key 不等于搜索登录。",
      "在 WorkBuddy 內確認官方帳號已登入，再執行原生搜尋。模型 API Key 不等於搜尋登入。",
      "WorkBuddy の公式アカウントでログイン後、検索を確認してください。モデル API Key は検索用ログインではありません。"),
    "deepseek-harness": copy(
      "Check the native search service credentials and balance in DSH. Its search service can differ from the model channel.",
      "在 DSH 中检查原生搜索服务的凭证和余额。搜索服务可能与模型渠道不同。",
      "在 DSH 中檢查原生搜尋服務的憑證和餘額。搜尋服務可能與模型管道不同。",
      "DSH の検索サービスの認証情報と残高を確認してください。モデルのチャネルとは別の場合があります。"),
    "grok-build": copy(
      "Native web_search needs a Responses provider that supports hosted search. Chat Completions conversion does not supply this capability.",
      "原生 web_search 需要支持托管搜索的 Responses 渠道。Chat Completions 协议转换不会补齐该能力。",
      "原生 web_search 需要支援託管搜尋的 Responses 管道。Chat Completions 協議轉換不會補齊該能力。",
      "web_search にはホスト型検索対応の Responses チャネルが必要です。Chat Completions 変換だけでは利用できません。"),
  };
  return <details className="agent-native-guide">
    <summary>{copy("Check native capabilities", "检查原生能力", "檢查原生能力", "ネイティブ機能の確認")}</summary>
    <dl>
      <dt>{copy("Tools · not verified", "工具调用 · 未验证", "工具呼叫 · 未驗證", "ツール呼び出し · 未検証")}</dt>
      <dd>{copy("Run a file read and a reversible edit in a test directory. Check the tool result and file contents.", "在测试目录执行一次文件读取和可恢复编辑，核对工具结果与文件内容。", "在測試目錄執行一次檔案讀取和可恢復編輯，核對工具結果與檔案內容。", "テスト用フォルダで読取と復元可能な編集を実行し、結果と内容を確認してください。")}</dd>
      <dt>{copy("Native search · not verified", "原生搜索 · 未验证", "原生搜尋 · 未驗證", "ネイティブ検索 · 未検証")}</dt>
      <dd>{search[agentId]}</dd>
      <dt>{copy("URL fetch · not verified", "网页读取 · 未验证", "網頁讀取 · 未驗證", "ページ取得 · 未検証")}</dt>
      <dd>{copy("Ask the Agent to fetch a public page and verify its contents. URL fetch and search are separate tools.", "让 Agent 读取公开网页并核对正文。网页读取与搜索是独立工具。", "讓 Agent 讀取公開網頁並核對正文。網頁讀取與搜尋是獨立工具。", "公開ページの取得と内容を確認してください。ページ取得と検索は別機能です。")}</dd>
      <dt>{copy("Compaction · not verified", "上下文压缩 · 未验证", "上下文壓縮 · 未驗證", "コンテキスト圧縮 · 未検証")}</dt>
      <dd>{copy("Use the Agent's native compaction control. Verify saved facts after compaction and after another turn. A text claim is not an event.", "使用 Agent 原生压缩入口，压缩后及下一轮分别检查关键事实是否保留。文字声称已压缩不算压缩事件。", "使用 Agent 原生壓縮入口，壓縮後及下一輪分別檢查關鍵事實是否保留。文字聲稱已壓縮不算壓縮事件。", "Agent の圧縮機能を使用し、圧縮後と次のターンで重要な情報を確認してください。テキストの宣言だけでは証明できません。")}</dd>
    </dl>
  </details>;
}
