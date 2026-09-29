import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { Check, Copy, Info } from "lucide-react";
import type { AgentRouteView, AgentUiMetadataView } from "../api";
import AgentCapabilityGuide from "./AgentCapabilityGuide";
import { useLocalizedCopy, type LocalizedCopy } from "./LanguageProvider";
import { useErrorToast } from "./ErrorToast";
import { Button } from "./ui/button";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "./ui/tooltip";
import "./AgentModelGuide.css";

interface ModelGuide {
  model?: string;
  provider?: string;
  instruction: string;
}

// Agent-facing names come from agent_integration/connectors/*.rs and cursor_tunnel.rs.
// These are picker entries, not upstream route targets. Preserve their exact spelling.
function modelGuide(agentId: string, copy: LocalizedCopy): ModelGuide | undefined {
  switch (agentId) {
    case "codex":
      return { model: "Token Station Auto", instruction: copy(
        "After connection, quit and reopen Codex. Find this entry in the model picker.",
        "接入后退出并重新打开 Codex，在模型选择器中找到此名称。",
        "接入後退出並重新開啟 Codex，在模型選擇器中找到此名稱。",
        "接続後に Codex を終了して開き直し、モデル選択でこの名前を探してください。",
      ) };
    case "claude-code":
      return { model: "Token Station Auto", instruction: copy(
        "After connection, restart Claude Code and use /model to find this entry. Its name includes a context suffix.",
        "接入后重启 Claude Code，输入 /model 查找此名称；名称后会附带上下文大小。",
        "接入後重新啟動 Claude Code，輸入 /model 尋找此名稱；名稱後會附帶上下文大小。",
        "接続後に Claude Code を再起動し、/model でこの名前を探してください。末尾にコンテキスト容量が付きます。",
      ) };
    case "opencode":
      return { provider: "token-station", model: "auto (智能路由)", instruction: copy(
        "After connection, select this provider and model in the OpenCode model picker.",
        "接入后在 OpenCode 的模型选择器中，选择以上供应商和模型。",
        "接入後在 OpenCode 的模型選擇器中，選擇以上供應商和模型。",
        "接続後に OpenCode のモデル選択で、このプロバイダーとモデルを選んでください。",
      ) };
    case "cursor":
      return { model: "Token Station Auto", instruction: copy(
        "Connect & launch selects this entry. Find it in the Cursor model picker.",
        "一键接入并启动后会选中此模型，可在 Cursor 的模型选择器中找到。",
        "一鍵接入並啟動後會選中此模型，可在 Cursor 的模型選擇器中找到。",
        "接続して起動すると、このモデルが選択されます。Cursor のモデル選択で確認できます。",
      ) };
    case "openclaw":
      return { provider: "tokenstation", model: "Token Station Auto", instruction: copy(
        "Connection sets the default model to tokenstation/auto. Restart OpenClaw to load the configuration.",
        "接入会将默认模型设为 tokenstation/auto，重启 OpenClaw 后加载配置。",
        "接入會將預設模型設為 tokenstation/auto，重新啟動 OpenClaw 後載入設定。",
        "接続時にデフォルトを tokenstation/auto に設定します。OpenClaw を再起動して設定を読み込んでください。",
      ) };
    case "workbuddy":
      return { provider: "Token Station", model: "Token Station Auto", instruction: copy(
        "After connection, select this entry in the WorkBuddy model picker.",
        "接入后在 WorkBuddy 的模型选择器中选择此模型。",
        "接入後在 WorkBuddy 的模型選擇器中選擇此模型。",
        "接続後に WorkBuddy のモデル選択で、このモデルを選んでください。",
      ) };
    case "kimi-code":
      return { model: "tokenstation-auto", instruction: copy(
        "Connection sets this as the default model. Start a new Kimi Code session to use it.",
        "接入会将此名称设为默认模型，重新启动 Kimi Code 会话后使用。",
        "接入會將此名稱設為預設模型，重新啟動 Kimi Code 工作階段後使用。",
        "接続時にこのモデルをデフォルトに設定します。新しい Kimi Code セッションで使用してください。",
      ) };
    case "deepseek-harness":
      return { provider: "Token Station", model: "Token Station Auto", instruction: copy(
        "Connection selects provider tokenstation and model auto by default. Start a new session to load them.",
        "接入会默认选择 tokenstation 供应商下的 auto 模型，重新启动会话后加载。",
        "接入會預設選擇 tokenstation 供應商下的 auto 模型，重新啟動工作階段後載入。",
        "接続時にプロバイダー tokenstation、モデル auto を選択します。新しいセッションで読み込んでください。",
      ) };
    case "grok-build":
      return { model: "Token Station Auto", instruction: copy(
        "Connection sets the default model to tokenstation. Campaign mode may still display its native Grok model name.",
        "接入会将默认模型设为 tokenstation；Campaign 模式仍可能显示原有 Grok 模型名称。",
        "接入會將預設模型設為 tokenstation；Campaign 模式仍可能顯示原有 Grok 模型名稱。",
        "接続時にデフォルトを tokenstation に設定します。Campaign モードでは元の Grok モデル名が表示される場合があります。",
      ) };
    case "nous-hermes-agent":
      return { provider: "custom", model: "auto", instruction: copy(
        "Connection configures custom / auto as the default. Start a new Hermes session to load it.",
        "接入会将 custom / auto 设为默认配置，重新启动 Hermes 会话后加载。",
        "接入會將 custom / auto 設為預設設定，重新啟動 Hermes 工作階段後載入。",
        "接続時に custom / auto をデフォルトに設定します。新しい Hermes セッションで読み込んでください。",
      ) };
    case "gemini-cli":
      return { instruction: copy(
        "After connection, restart Gemini CLI and use its existing model options. No Token Station model entry is added.",
        "接入后重启 Gemini CLI，继续使用原有模型选项，不会新增名为 Token Station 的模型。",
        "接入後重新啟動 Gemini CLI，繼續使用原有模型選項，不會新增名為 Token Station 的模型。",
        "接続後に Gemini CLI を再起動し、既存のモデルを使用してください。Token Station というモデルは追加されません。",
      ) };
    case "claude-desktop":
      return { instruction: copy(
        "After connection, quit and reopen Claude Desktop. Use its existing model options through the configured gateway.",
        "接入后退出并重新打开 Claude Desktop，继续使用原有模型选项，请求会经过已配置的网关。",
        "接入後退出並重新開啟 Claude Desktop，繼續使用原有模型選項，請求會經過已設定的閘道。",
        "接続後に Claude Desktop を終了して開き直してください。既存のモデルを設定済みゲートウェイ経由で使用します。",
      ) };
    default:
      return undefined;
  }
}

export default function AgentModelGuide({ metadata, connected, route }: {
  metadata: Pick<AgentUiMetadataView, "agent_id" | "display_name">;
  connected: boolean;
  route?: AgentRouteView;
}) {
  const { copy } = useLocalizedCopy();
  const { showError } = useErrorToast();
  const [copiedResult, setCopiedResult] = useState<{ agentId: string; request: number } | null>(null);
  const copyRequest = useRef(0);
  const latestCopy = useRef(copy);
  const guide = modelGuide(metadata.agent_id, copy);
  const copied = copiedResult?.agentId === metadata.agent_id;

  useLayoutEffect(() => {
    latestCopy.current = copy;
  }, [copy]);

  useLayoutEffect(() => {
    setCopiedResult(null);
    return () => { copyRequest.current += 1; };
  }, [metadata.agent_id]);

  useEffect(() => {
    if (copiedResult === null) return;
    const timer = window.setTimeout(() => setCopiedResult(null), 1600);
    return () => window.clearTimeout(timer);
  }, [copiedResult]);

  if (!guide) return null;
  const name = metadata.display_name;
  const heading = connected
    ? copy(`Use in ${name}`, `在 ${name} 中使用`, `在 ${name} 中使用`, `${name} で使用`)
    : copy(`Select in ${name} after connection`, `接入后在 ${name} 中选择`, `接入後在 ${name} 中選擇`, `接続後に ${name} で選択`);

  async function copyModel() {
    if (!guide?.model) return;
    const request = ++copyRequest.current;
    try {
      await navigator.clipboard.writeText(guide.model);
      if (request !== copyRequest.current) return;
      setCopiedResult({ agentId: metadata.agent_id, request });
    } catch {
      if (request !== copyRequest.current) return;
      setCopiedResult(null);
      showError(latestCopy.current(
        "Unable to copy the model name. Select and copy it manually.",
        "无法复制模型名称，请手动选中并复制。",
        "無法複製模型名稱，請手動選取並複製。",
        "モデル名をコピーできません。選択して手動でコピーしてください。",
      ), "agent-model-guide-copy");
    }
  }

  return (
    <section className="agent-model-guide agent-flat-surface" aria-label={heading}>
      <h3 className="sr-only">{heading}</h3>
      <div className="agent-model-guide-primary">
      <span className="agent-model-guide-label" aria-hidden="true">{connected
        ? copy("Model entry", "模型入口", "模型入口", "モデル入口")
        : copy("Select after connection", "接入后选择", "接入後選擇", "接続後に選択")}</span>
      <dl className="agent-model-guide-values">
        {guide.provider && <div><dt>{copy("Provider", "供应商", "供應商", "プロバイダー")}</dt><dd>{guide.provider}</dd></div>}
        <div>
          <dt>{copy("Model", "模型", "模型", "モデル")}</dt>
          <dd>
            <strong>{guide.model ?? copy("Keep the existing model options", "沿用原有模型选项", "沿用原有模型選項", "既存のモデル選択を使用")}</strong>
            {guide.model && <Button variant="ghost" size="icon-sm" type="button" onClick={() => void copyModel()}
              aria-label={copy("Copy model name", "复制模型名称", "複製模型名稱", "モデル名をコピー")}
              title={copy("Copy model name", "复制模型名称", "複製模型名稱", "モデル名をコピー")}>
              {copied ? <Check aria-hidden="true" /> : <Copy aria-hidden="true" />}
            </Button>}
          </dd>
        </div>
      </dl>
      </div>
      <TooltipProvider>
        <Tooltip>
          <TooltipTrigger asChild>
            <Button className="agent-model-guide-help" variant="ghost" size="icon-sm" type="button"
              aria-label={copy("View usage instructions", "查看使用说明", "檢視使用說明", "使用方法を表示")}>
              <Info aria-hidden="true" />
            </Button>
          </TooltipTrigger>
          <TooltipContent side="bottom" className="agent-model-guide-tooltip">{guide.instruction}</TooltipContent>
        </Tooltip>
      </TooltipProvider>
      {route && <AgentCapabilityGuide route={route} />}
      <span className="sr-only" role="status">{copied ? copy("Copied", "已复制", "已複製", "コピーしました") : ""}</span>
    </section>
  );
}
