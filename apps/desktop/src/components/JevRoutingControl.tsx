import { useCallback, useEffect, useId, useRef, useState } from "react";
import {
  clearJevKey, getJevStatus, saveJevKey, setJevEnabled, testJevConnection,
  type JevStatus,
} from "../api";
import { useLocalizedCopy } from "./LanguageProvider";
import { Button } from "./ui/button";
import { Input } from "./ui/input";
import RoutingClassifierCard from "./RoutingClassifierCard";
import "./JevRoutingControl.css";

type Operation = "save" | "remove" | "enable" | "test";
type Copy = ReturnType<typeof useLocalizedCopy>["copy"];

function outcomeMessage(outcome: string | null, copy: Copy): string | null {
  switch (outcome) {
    case "applied": return copy("Jev selected the routing tier.", "已使用 Jev 选择的档位。", "已使用 Jev 選擇的檔位。", "Jev が選択した分層を使用しました。");
    case "overridden": return copy("An explicit route or rule took priority.", "显式路由或规则优先，本次跳过 Jev。", "明確路由或規則優先，本次略過 Jev。", "明示的なルートまたはルールを優先しました。");
    case "local_only": return copy("Local-only request. No text was sent to Jev.", "仅本地请求，未向 Jev 发送文本。", "僅本機請求，未向 Jev 傳送文字。", "ローカル限定のリクエストです。Jev にテキストを送信していません。");
    case "unsupported": return copy("This input is not supported. Existing rules were used.", "不支持此输入，已使用原有规则。", "不支援此輸入，已使用原有規則。", "この入力には対応していません。既存のルールを使用しました。");
    case "missing_key": return copy("Save a Jev key before enabling cloud routing.", "请先保存 Jev Key，再启用云端分档。", "請先儲存 Jev Key，再啟用雲端分檔。", "クラウド分層を有効にする前に Jev キーを保存してください。");
    case "timeout": return copy("Jev timed out. Existing rules were used.", "Jev 超时，已使用原有规则。", "Jev 逾時，已使用原有規則。", "Jev がタイムアウトしました。既存のルールを使用しました。");
    case "cancelled": return copy("Classification was cancelled. Its result was not used.", "分档已取消，未使用其结果。", "分檔已取消，未使用其結果。", "分類をキャンセルしました。結果は使用していません。");
    case "busy": return copy("Jev was busy. Existing rules were used.", "Jev 正忙，已使用原有规则。", "Jev 忙碌中，已使用原有規則。", "Jev が処理中のため、既存のルールを使用しました。");
    case "low_confidence": return copy("Jev confidence was too low. Existing rules were used.", "Jev 置信度不足，已使用原有规则。", "Jev 信心度不足，已使用原有規則。", "Jev の信頼度が不足しています。既存のルールを使用しました。");
    case "invalid": return copy("Jev returned an invalid result. Existing rules were used.", "Jev 返回无效结果，已使用原有规则。", "Jev 傳回無效結果，已使用原有規則。", "Jev の結果が無効です。既存のルールを使用しました。");
    case "unauthorized": return copy("The Jev key is invalid or unauthorized. Replace the key.", "Jev Key 无效或未获授权，请更换 Key。", "Jev Key 無效或未獲授權，請更換 Key。", "Jev キーが無効か、権限がありません。キーを変更してください。");
    case "rate_limited": return copy("Jev rate limit reached. Existing rules were used.", "Jev 已限流，已使用原有规则。", "Jev 已限流，已使用原有規則。", "Jev のレート制限に達しました。既存のルールを使用しました。");
    case "unavailable": return copy("Jev is unavailable. Existing rules were used.", "Jev 暂不可用，已使用原有规则。", "Jev 暫時無法使用，已使用原有規則。", "Jev を利用できません。既存のルールを使用しました。");
    case "no_route": return copy("The selected tier has no usable route. Existing rules were used.", "所选档位没有可用路由，已使用原有规则。", "所選檔位沒有可用路由，已使用原有規則。", "選択された分層に利用可能なルートがありません。既存のルールを使用しました。");
    case null:
    case "ready":
    case "disabled": return null;
    default: return copy("Jev classification failed. Existing rules were used.", "Jev 分档失败，已使用原有规则。", "Jev 分檔失敗，已使用原有規則。", "Jev の分類に失敗しました。既存のルールを使用しました。");
  }
}

export default function JevRoutingControl({ onEnabledChange }: { onEnabledChange?: (enabled: boolean) => void }) {
  const { copy } = useLocalizedCopy();
  const id = useId();
  const [status, setStatus] = useState<JevStatus | null>(null);
  const [apiKey, setApiKey] = useState("");
  const [pending, setPending] = useState<Operation | null>(null);
  const [failed, setFailed] = useState<Operation | null>(null);
  const [completed, setCompleted] = useState<Operation | null>(null);
  const [readFailed, setReadFailed] = useState(false);
  const [reading, setReading] = useState(false);
  const statusRef = useRef<JevStatus | null>(null);
  const mounted = useRef(false);
  const mutating = useRef(false);
  const polling = useRef<number | null>(null);
  const revision = useRef(0);
  const enabledCallback = useRef(onEnabledChange);
  enabledCallback.current = onEnabledChange;

  const acceptStatus = useCallback((next: JevStatus) => {
    statusRef.current = next;
    setStatus(next);
    setReadFailed(false);
    enabledCallback.current?.(next.enabled);
  }, []);

  const poll = useCallback(async () => {
    if (polling.current === revision.current || mutating.current) return;
    const startedAt = revision.current;
    polling.current = startedAt;
    setReading(true);
    try {
      const next = await getJevStatus();
      if (mounted.current && startedAt === revision.current) {
        acceptStatus(next);
        setCompleted(null);
      }
    } catch {
      if (mounted.current && startedAt === revision.current) setReadFailed(true);
    } finally {
      if (polling.current === startedAt) polling.current = null;
      if (mounted.current && startedAt === revision.current) setReading(false);
    }
  }, [acceptStatus]);

  useEffect(() => {
    mounted.current = true;
    void poll();
    const timer = window.setInterval(() => {
      if (statusRef.current?.enabled && !document.hidden) void poll();
    }, 2_000);
    return () => {
      mounted.current = false;
      revision.current += 1;
      window.clearInterval(timer);
    };
  }, [poll]);

  const mutate = async (operation: Operation, action: () => Promise<JevStatus>) => {
    if (mutating.current || !statusRef.current) return;
    mutating.current = true;
    const startedAt = ++revision.current;
    setPending(operation);
    setReading(false);
    setFailed(null);
    setCompleted(null);
    try {
      const next = await action();
      if (mounted.current && startedAt === revision.current) {
        acceptStatus(next);
        setCompleted(operation);
        if (operation === "save" || operation === "remove") setApiKey("");
      }
    } catch {
      if (mounted.current && startedAt === revision.current) {
        setFailed(operation);
        // A failed probe records an outcome. Key removal can disable routing before storage fails.
        try {
          const next = await getJevStatus();
          if (mounted.current && startedAt === revision.current) acceptStatus(next);
        } catch {
          if (mounted.current && startedAt === revision.current) setReadFailed(true);
        }
      }
    } finally {
      mutating.current = false;
      if (mounted.current && startedAt === revision.current) setPending(null);
    }
  };

  const submitKey = () => {
    const key = apiKey.trim();
    if (key) void mutate("save", () => saveJevKey(key));
  };
  const title = copy("Jev cloud smart tiers", "Jev 云端智能分档", "Jev 雲端智慧分檔", "Jev クラウドスマート分層");
  const locked = pending !== null || status === null;
  const outcome = outcomeMessage(status?.last_outcome ?? null, copy);
  const failedMessage = failed === "enable"
    ? copy("Setting was not saved. Try again.", "设置未保存，请重试。", "設定未儲存，請重試。", "設定を保存できませんでした。再試行してください。")
    : failed === "save"
      ? copy("The key was not saved. Try again.", "Key 未保存，请重试。", "Key 未儲存，請重試。", "キーを保存できませんでした。再試行してください。")
      : failed === "remove"
        ? copy("The key was not removed. Try again.", "Key 未移除，请重试。", "Key 未移除，請重試。", "キーを削除できませんでした。再試行してください。")
        : failed === "test"
          ? copy("Connection test failed. Check the key and network, then retry.", "连接测试失败，请检查 Key 和网络后重试。", "連線測試失敗，請檢查 Key 和網路後重試。", "接続テストに失敗しました。キーとネットワークを確認して再試行してください。")
          : null;
  const testPassed = completed === "test" && (status?.last_outcome === "ready" || status?.last_outcome === "applied");
  const needsKey = status?.last_outcome === "unauthorized" || status?.last_outcome === "missing_key";

  return (
    <RoutingClassifierCard
      id={id} title={title}
      summary={copy("Text to TypeSafe · Billed API", "文本发至 TypeSafe · 按量计费", "文字傳至 TypeSafe · 按量計費", "TypeSafe にテキスト送信 · 従量課金")}
      statusLabel={status
        ? status.has_key
          ? copy("Key saved", "已配置 Key", "已設定 Key", "キー設定済み")
          : copy("No key configured", "未配置 Key", "未設定 Key", "キー未設定")
        : readFailed
          ? copy("Status unavailable", "状态不可用", "狀態無法取得", "状態を取得できません")
          : copy("Reading status…", "正在读取状态…", "正在讀取狀態…", "状態を取得中…")}
      settingsLabel={copy("Jev settings", "Jev 设置", "Jev 設定", "Jev 設定")}
      enabled={status?.enabled ?? false} disabled={locked || (!status?.has_key && !status?.enabled)}
      busy={pending === "enable"} describedBy={`${id}-disclosure ${id}-fallback`}
      onEnabledChange={(enabled) => void mutate("enable", () => setJevEnabled(enabled))}
      feedback={(failedMessage || needsKey || readFailed) && <div className="jev-routing-error" role="alert">
        {failedMessage && <p>{failedMessage}</p>}
        {needsKey && <p>{outcome}</p>}
        {readFailed && <div className="jev-routing-retry"><p>{copy("Cannot read Jev status. Retry to load the controls.", "无法读取 Jev 状态，请重试。", "無法讀取 Jev 狀態，請重試。", "Jev の状態を取得できません。再試行してください。")}</p>
          <Button type="button" variant="outline" size="sm" disabled={reading || pending !== null} onClick={() => void poll()}>{copy("Retry", "重试", "重試", "再試行")}</Button>
        </div>}
      </div>}
    >
      <p className="jev-routing-note" id={`${id}-disclosure`}>{copy(
        "When enabled, bounded user and assistant text is sent to TypeSafe. Separate API charges apply. Local-only requests skip Jev.",
        "启用后，有长度上限的用户和助手文本会发送给 TypeSafe，产生独立 API 费用。仅本地请求会跳过 Jev。",
        "啟用後，有長度上限的使用者和助手文字會傳送給 TypeSafe，產生獨立 API 費用。僅本機請求會略過 Jev。",
        "有効にすると、長さを制限したユーザーとアシスタントのテキストを TypeSafe に送信します。別途 API 料金が発生します。ローカル限定のリクエストは対象外です。",
      )}</p>
      <p className="jev-routing-note" id={`${id}-fallback`}>{copy(
        "Jev takes priority over local classification. Timeout, failure, or low confidence uses existing rules without calling the local classifier.",
        "Jev 优先于本地分档。超时、失败或低置信度时使用原有规则，不再调用本地分档。",
        "Jev 優先於本機分檔。逾時、失敗或低信心度時使用原有規則，不再呼叫本機分檔。",
        "Jev はローカル分類より優先されます。タイムアウト、失敗、信頼度不足の場合、ローカル分類を呼ばずに既存のルールを使用します。",
      )}</p>
      <form className="jev-routing-key-form" onSubmit={(event) => { event.preventDefault(); submitKey(); }}>
        <label className="jev-routing-key-label" htmlFor={`${id}-key`}>Jev API Key</label>
        <div className="jev-routing-key-actions">
          <Input
            id={`${id}-key`} type="password" autoComplete="off" spellCheck={false}
            value={apiKey} disabled={locked} aria-describedby={`${id}-storage`}
            placeholder={status?.has_key
              ? copy("Enter a replacement key", "输入新的 Key 以替换", "輸入新的 Key 以取代", "変更するキーを入力")
              : copy("Enter your Jev API key", "输入你的 Jev API Key", "輸入你的 Jev API Key", "Jev API キーを入力")}
            onChange={(event) => setApiKey(event.target.value)}
          />
          <Button type="submit" disabled={locked || !apiKey.trim()}>{copy("Save key", "保存 Key", "儲存 Key", "キーを保存")}</Button>
          <Button type="button" variant="outline" disabled={locked || !status?.has_key} onClick={() => void mutate("test", testJevConnection)}>{copy("Test connection", "测试连接", "測試連線", "接続テスト")}</Button>
          <Button type="button" variant="ghost" disabled={locked || !status?.has_key} onClick={() => void mutate("remove", clearJevKey)}>{copy("Remove key", "移除 Key", "移除 Key", "キーを削除")}</Button>
        </div>
      </form>
      <p className="jev-routing-note" id={`${id}-storage`}>{copy(
        "The key is stored as plaintext in private local storage, not the system Keychain. Tests send synthetic text only and do not enable routing.",
        "Key 以明文保存在权限受限的本地存储中，不是系统钥匙串。测试仅发送合成文本，不会启用路由。",
        "Key 以明文儲存在權限受限的本機儲存空間中，不是系統鑰匙圈。測試僅傳送合成文字，不會啟用路由。",
        "キーはアクセス制限付きのローカル領域に平文で保存されます。システムのキーチェーンではありません。テストは合成テキストのみを送信し、ルーティングを有効にしません。",
      )}</p>
      {status && <p className="jev-routing-note jev-routing-settings">{copy(
        `${status.model} · Deadline ${status.timeout_ms} ms · Confidence ≥ ${Math.round(status.confidence_threshold * 100)}%`,
        `${status.model} · 超时 ${status.timeout_ms} ms · 置信度 ≥ ${Math.round(status.confidence_threshold * 100)}%`,
        `${status.model} · 逾時 ${status.timeout_ms} ms · 信心度 ≥ ${Math.round(status.confidence_threshold * 100)}%`,
        `${status.model} · タイムアウト ${status.timeout_ms} ms · 信頼度 ≥ ${Math.round(status.confidence_threshold * 100)}%`,
      )}</p>}
      <div className="jev-routing-status" role="status" aria-live="polite">
        {pending
          ? copy("Updating Jev…", "正在处理 Jev 操作…", "正在處理 Jev 操作…", "Jev の操作を処理中…")
          : <>
              {testPassed && <p>{copy("Connection test passed.", "连接测试通过。", "連線測試通過。", "接続テストに成功しました。")}</p>}
              {status && <p>{status.enabled
                ? copy("Jev is enabled and takes priority over local classification.", "Jev 已启用，优先于本地分档。", "Jev 已啟用，優先於本機分檔。", "Jev は有効です。ローカル分類より優先されます。")
                : copy("Jev is off. Your existing routing remains active.", "Jev 已关闭，继续使用现有路由。", "Jev 已關閉，繼續使用現有路由。", "Jev は無効です。既存のルーティングを使用します。")}</p>}
              {outcome && !needsKey && <p>{outcome}</p>}
              {status?.last_outcome === "applied" && status.last_tier && <p>{copy("Last tier", "最近档位", "最近檔位", "直近の分層")}: {status.last_tier === "low"
                ? copy("Low", "低档", "低檔", "低")
                : status.last_tier === "medium"
                  ? copy("Medium", "中档", "中檔", "中")
                  : copy("High", "高档", "高檔", "高")}</p>}
              {status?.last_latency_ms != null && <p>{copy("Last latency", "最近耗时", "最近耗時", "直近の処理時間")}: {status.last_latency_ms} ms</p>}
            </>}
      </div>
    </RoutingClassifierCard>
  );
}
