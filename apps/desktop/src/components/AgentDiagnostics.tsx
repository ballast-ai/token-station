import { useEffect, useRef, useState } from "react";
import { cancelModelTestChat, getAgentRequestEvidence, testAgentRoute, type AgentConnectionLimitsView, type AgentId, type ReceiptView } from "../api";
import { useLocalizedCopy } from "./LanguageProvider";
import { Button } from "./ui/button";
import AgentCapabilityGuide from "./AgentCapabilityGuide";

interface Props {
  agentId: AgentId;
  configured: boolean;
  runningRevision?: number | null;
  modelLimits?: AgentConnectionLimitsView | null;
}

export default function AgentDiagnostics({ agentId, configured, runningRevision, modelLimits }: Props) {
  const { copy } = useLocalizedCopy();
  const [receipt, setReceipt] = useState<ReceiptView | null>(null);
  const [phase, setPhase] = useState<"idle" | "loading" | "ready" | "error">("idle");
  const generation = useRef(0);
  const activeProbe = useRef<string | null>(null);
  const [probe, setProbe] = useState<"idle" | "running" | "success" | "error" | "cancelled">("idle");
  const [cancelError, setCancelError] = useState(false);
  useEffect(() => {
    generation.current += 1;
    setReceipt(null);
    setPhase("idle");
    setProbe("idle");
    setCancelError(false);
    return () => {
      generation.current += 1;
      if (activeProbe.current) void cancelModelTestChat(activeProbe.current).catch(() => {});
      activeProbe.current = null;
    };
  }, [agentId, configured, runningRevision]);

  async function refresh() {
    const current = ++generation.current;
    setPhase("loading");
    setReceipt(null);
    try {
      const items = await getAgentRequestEvidence(agentId);
      if (generation.current !== current) return;
      setReceipt(items.filter((item) => item.agent_id === agentId
        && ["chat_completions", "responses", "messages", "gemini_generate_content"].includes(item.path_kind ?? ""))
        .sort((a, b) => b.started_at_ms - a.started_at_ms)[0] ?? null);
      setPhase("ready");
    } catch {
      if (generation.current === current) setPhase("error");
    }
  }

  async function runProbe() {
    const id = `agent-probe-${crypto.randomUUID()}`;
    activeProbe.current = id;
    setProbe("running");
    setCancelError(false);
    try {
      await testAgentRoute(agentId, id);
      if (activeProbe.current === id) setProbe("success");
    } catch {
      if (activeProbe.current === id) setProbe("error");
    } finally {
      if (activeProbe.current === id) activeProbe.current = null;
    }
  }

  async function stopProbe() {
    const id = activeProbe.current;
    if (!id) return;
    try {
      await cancelModelTestChat(id);
      if (activeProbe.current === id) {
        activeProbe.current = null;
        setProbe("cancelled");
        setCancelError(false);
      }
    } catch {
      if (activeProbe.current === id) setCancelError(true);
    }
  }

  const finalAttempt = receipt?.attempt_records[receipt.attempt_records.length - 1];
  const actual = receipt?.routing ?? finalAttempt;
  const succeeded = receipt && receipt.status >= 200 && receipt.status < 300 && !receipt.error_code
    && (!receipt.stream || finalAttempt?.stream_outcome === "complete");
  return (
    <section className="agent-connection-detail agent-flat-surface agent-diagnostics" aria-label={copy("Connection diagnostics", "接入诊断", "連線診斷", "接続診断")}>
      <h3>{copy("Connection diagnostics", "接入诊断", "連線診斷", "接続診断")}</h3>
      <p>{configured
        ? copy("Configuration written; native capabilities are not verified", "配置已写入，原生能力尚未验证", "設定已寫入，原生能力尚未驗證", "設定済み。ネイティブ機能は未検証です")
        : copy("Configuration is not connected", "尚未接入配置", "尚未連線設定", "設定は未接続です")}</p>
      <Button variant="outline" disabled={phase === "loading" || probe === "running"} onClick={() => void refresh()}>
        {phase === "loading" ? copy("Checking…", "检查中…", "檢查中…", "確認中…") : copy("Check recent requests", "检查最近请求", "檢查最近請求", "最近のリクエストを確認")}
      </Button>
      <Button variant="outline" disabled={runningRevision == null || probe === "running"} onClick={() => void runProbe()}>
        {copy("Test current Agent route", "测试当前 Agent 路由", "測試目前 Agent 路由", "現在の Agent ルートをテスト")}
      </Button>
      {probe === "running" && <Button variant="ghost" onClick={() => void stopProbe()}>{copy("Cancel test", "取消测试", "取消測試", "テストを中止")}</Button>}
      <p>{copy("Sends one short message through the running Agent route. Provider usage is billed normally. This does not launch the Agent.", "通过当前运行的 Agent 路由发送一条短消息，按渠道正常计费。此操作不会启动 Agent。", "透過目前執行的 Agent 路由傳送一條短訊息，按管道正常計費。此操作不會啟動 Agent。", "実行中の Agent ルートへ短いメッセージを送信します。通常の利用料金が発生します。Agent は起動しません。")}</p>
      <div role="status" aria-live="polite">
        {cancelError && probe === "running" && <p>{copy("Cancellation failed. The test is still running. Retry cancellation or wait for the timeout.", "取消请求未成功。测试仍在运行，可重试取消或等待超时。", "取消請求未成功。測試仍在執行，可重試取消或等待逾時。", "中止要求が失敗しました。再試行するかタイムアウトを待ってください。")}</p>}
        {probe === "running" && <p>{copy("Testing the running route…", "正在测试运行中的路由…", "正在測試執行中的路由…", "実行中のルートをテストしています…")}</p>}
        {probe === "success" && <p>{copy("Route probe succeeded; native Agent capabilities still need separate verification.", "路由探测成功；Agent 原生能力仍需单独验证。", "路由探測成功；Agent 原生能力仍需單獨驗證。", "ルートテスト成功。Agent のネイティブ機能は別途検証が必要です。")}</p>}
        {probe === "error" && <p>{copy("Route probe failed. Check recent requests for the channel, model, and error.", "路由探测失败。请检查最近请求，查看渠道、模型和错误。", "路由探測失敗。請檢查最近請求，查看管道、模型和錯誤。", "ルートテスト失敗。最近の記録でチャネル、モデル、エラーを確認してください。")}</p>}
        {probe === "cancelled" && <p>{copy("Route probe cancelled", "已取消路由测试", "已取消路由測試", "ルートテストを中止しました")}</p>}
        {phase === "error" && <p>{copy("Cannot read request records. Try again.", "无法读取请求记录，请重试。", "無法讀取請求記錄，請重試。", "記録を読み取れません。再試行してください。")}</p>}
        {phase === "ready" && !receipt && <p>{copy("No recent model request was found. Send a message in the Agent, then check again.", "未找到最近的模型请求。请在 Agent 中发送消息后重新检查。", "未找到最近的模型請求。請在 Agent 中傳送訊息後重新檢查。", "モデルリクエストがありません。Agent で送信後に再確認してください。")}</p>}
        {receipt && <>
          <p>{succeeded ? copy("Latest request succeeded", "最近请求成功", "最近請求成功", "直近のリクエストが成功") : copy("Latest request failed or was interrupted", "最近请求失败或中断", "最近請求失敗或中斷", "直近のリクエストが失敗または中断")}</p>
          <dl className="agent-connection-facts">
            <div><dt>{copy("Observed channel / model", "实际渠道 / 模型", "實際管道 / 模型", "実際のチャネル / モデル")}</dt><dd style={{ overflowWrap: "anywhere" }}>{actual ? `${actual.upstream} / ${actual.model}` : copy("No successful route recorded", "未记录成功路由", "未記錄成功路由", "成功ルートの記録なし")}</dd></div>
            <div><dt>{copy("Request time", "请求时间", "請求時間", "リクエスト時刻")}</dt><dd>{new Date(receipt.started_at_ms).toLocaleString()}</dd></div>
            <div><dt>{copy("Request / running revision", "请求 / 当前运行版本", "請求 / 目前執行版本", "リクエスト / 現在の実行版")}</dt><dd>{receipt.running_revision ?? "—"} / {runningRevision ?? "—"}</dd></div>
            <div><dt>HTTP</dt><dd>{receipt.status}{receipt.error_code ? ` · ${receipt.error_code}` : ""}</dd></div>
          </dl>
          {(receipt.running_revision == null || runningRevision == null || receipt.running_revision !== runningRevision) && <p>{copy("This request used another running revision. It cannot verify the current route.", "此请求来自其他运行版本，不能验证当前路由。", "此請求來自其他執行版本，不能驗證目前路由。", "別の実行版の記録です。現在のルートを検証できません。")}</p>}
        </>}
      </div>
      <p>{copy("Tools, search, URL fetch, and compaction need verification inside the Agent. A successful request does not prove these capabilities work.", "工具、搜索、网页读取和压缩需要在 Agent 内验证。请求成功不代表这些能力可用。", "工具、搜尋、網頁讀取和壓縮需要在 Agent 內驗證。請求成功不代表這些能力可用。", "ツール、検索、ページ取得、圧縮は Agent 内で検証が必要です。リクエスト成功だけでは確認できません。")}</p>
      <AgentCapabilityGuide agentId={agentId} />
      {modelLimits && <details className="agent-native-guide">
        <summary>{copy("Model token limits", "模型 Token 上限", "模型 Token 上限", "モデルのトークン上限")}</summary>
        <dl><dt>{copy("Context / output / input", "上下文 / 输出 / 输入", "上下文 / 輸出 / 輸入", "コンテキスト / 出力 / 入力")}</dt>
          <dd>{modelLimits.context} / {modelLimits.output} / {modelLimits.max_input}</dd></dl>
        <p>{modelLimits.source === "compatibility"
          ? copy("Compatibility defaults are in use. Confirm the channel's limits before long sessions.", "正在使用兼容默认值。长会话前请核对渠道实际限制。", "正在使用相容預設值。長對話前請核對管道實際限制。", "互換用の既定値です。長いセッションの前にチャネルの上限を確認してください。")
          : copy("From model configuration; maximum capacity is not tested. A short-message probe cannot verify this limit.", "来自模型配置；最大容量尚未实测。短消息探测不能验证此上限。", "來自模型設定；最大容量尚未實測。短訊息探測不能驗證此上限。", "モデル設定の値です。最大容量は未検証です。短いメッセージのテストでは確認できません。")}</p>
      </details>}
    </section>
  );
}
