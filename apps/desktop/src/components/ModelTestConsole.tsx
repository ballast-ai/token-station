import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import { Bot, LoaderCircle, MessageSquareText, Route, SendHorizontal, Square, Trash2 } from "lucide-react";
import type { ModelTestMessage, StateView } from "../api";
import { cancelModelTestChat, testModelChatStream } from "../api";
import { useLocalizedCopy } from "./LanguageProvider";
import { Button } from "./ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "./ui/dialog";
interface ModelTestConsoleProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  routingMode: StateView["routing_mode"];
  routeState: "running" | "draft";
}

export type TranscriptItem = ModelTestMessage & {
  id: number;
  turnId: string;
  latencyMs?: number;
  firstTokenMs?: number;
  streaming?: boolean;
  stopped?: boolean;
  errorMessage?: string;
};

const MODEL_TEST_MAX_MESSAGES = 20;
const MODEL_TEST_MAX_MESSAGE_BYTES = 16_000;
const MODEL_TEST_MAX_TOTAL_BYTES = 64_000;
const MODEL_TEST_MAX_TRANSCRIPT_TURNS = 10;
const textEncoder = new TextEncoder();

type ModelTestRequestMessages = {
  messages: ModelTestMessage[];
  error: "message_too_large" | null;
};

export function buildModelTestRequestMessages(
  items: readonly TranscriptItem[],
  prompt: string,
): ModelTestRequestMessages {
  const promptBytes = textEncoder.encode(prompt).byteLength;
  if (promptBytes > MODEL_TEST_MAX_MESSAGE_BYTES) {
    return { messages: [], error: "message_too_large" };
  }

  const turns = new Map<string, { user?: TranscriptItem; assistant?: TranscriptItem }>();
  items.forEach((item) => {
    const turn = turns.get(item.turnId) ?? {};
    if (item.role === "user" && !turn.user) turn.user = item;
    if (item.role === "assistant" && !turn.assistant) turn.assistant = item;
    turns.set(item.turnId, turn);
  });
  const completedTurns = [...turns.values()].filter((turn): turn is {
    user: TranscriptItem;
    assistant: TranscriptItem;
  } => Boolean(
    turn.user
    && turn.assistant
    && !turn.assistant.errorMessage
    && !turn.assistant.stopped
    && !turn.assistant.streaming
    && turn.assistant.content.trim(),
  ));

  const messages: ModelTestMessage[] = [{ role: "user", content: prompt }];
  let totalBytes = promptBytes;
  for (let index = completedTurns.length - 1; index >= 0; index -= 1) {
    const { user, assistant } = completedTurns[index];
    const userBytes = textEncoder.encode(user.content).byteLength;
    const assistantBytes = textEncoder.encode(assistant.content).byteLength;
    if (
      userBytes > MODEL_TEST_MAX_MESSAGE_BYTES
      || assistantBytes > MODEL_TEST_MAX_MESSAGE_BYTES
      || messages.length + 2 > MODEL_TEST_MAX_MESSAGES
      || totalBytes + userBytes + assistantBytes > MODEL_TEST_MAX_TOTAL_BYTES
    ) break;
    messages.unshift(
      { role: "user", content: user.content },
      { role: "assistant", content: assistant.content },
    );
    totalBytes += userBytes + assistantBytes;
  }
  return { messages, error: null };
}

export function trimModelTestTranscript(items: readonly TranscriptItem[]): TranscriptItem[] {
  const retainedTurnIds = new Set<string>();
  for (let index = items.length - 1; index >= 0; index -= 1) {
    retainedTurnIds.add(items[index].turnId);
    if (retainedTurnIds.size === MODEL_TEST_MAX_TRANSCRIPT_TURNS) break;
  }
  return items.filter((item) => retainedTurnIds.has(item.turnId));
}

type ActiveRequest = {
  requestId: string;
  assistantId: number;
  stopped: boolean;
  cancelling: boolean;
  cancelPromise: Promise<boolean> | null;
  flushTimer: number | null;
  pendingDelta: string;
  pendingFirstTokenMs: number | null;
};

function errorMessage(error: unknown) {
  if (error instanceof Error && error.message.trim()) return error.message;
  if (typeof error === "string" && error.trim()) return error;
  return "Model request failed";
}

export default function ModelTestConsole({
  open,
  onOpenChange,
  routingMode,
  routeState,
}: ModelTestConsoleProps) {
  const { copy } = useLocalizedCopy();
  const composerRef = useRef<HTMLTextAreaElement>(null);
  const transcriptRef = useRef<HTMLDivElement>(null);
  const nextId = useRef(1);
  const nextRequestId = useRef(1);
  const activeRequestRef = useRef<ActiveRequest | null>(null);
  const [items, setItems] = useState<TranscriptItem[]>([]);
  const [draft, setDraft] = useState("");
  const [composerError, setComposerError] = useState<string | null>(null);
  const [sending, setSending] = useState(false);
  const [cancelling, setCancelling] = useState(false);
  const focusComposer = (delay = 60) => window.setTimeout(() => composerRef.current?.focus(), delay);
  const routeModeLabel = routingMode === "direct"
    ? copy("Direct", "简单路由", "簡單路由", "ダイレクト")
    : routingMode === "quota_first"
      ? copy("Quota-first", "额度优先", "額度優先", "クォータ優先")
      : copy("Smart routing", "智能路由", "智慧路由", "スマートルーティング");
  const routeStateLabel = routeState === "running"
    ? copy("Running global route", "运行中的全局路由", "執行中的全域路由", "実行中のグローバルルート")
    : copy("Draft global route", "草稿全局路由", "草稿全域路由", "下書きのグローバルルート");

  const takePendingDelta = (request: ActiveRequest) => {
    if (request.flushTimer != null) window.clearTimeout(request.flushTimer);
    request.flushTimer = null;
    const pending = {
      delta: request.pendingDelta,
      firstTokenMs: request.pendingFirstTokenMs,
    };
    request.pendingDelta = "";
    request.pendingFirstTokenMs = null;
    return pending;
  };

  const cancelActiveRequest = async (preservePartial: boolean): Promise<boolean> => {
    const active = activeRequestRef.current;
    if (!active) return true;
    if (active.cancelPromise) return active.cancelPromise;
    active.cancelling = true;
    setCancelling(true);
    const cancellation = cancelModelTestChat(active.requestId).then(() => {
      const pending = takePendingDelta(active);
      active.stopped = true;
      if (activeRequestRef.current === active) activeRequestRef.current = null;
      setSending(false);
      setCancelling(false);
      if (preservePartial) {
        setItems((current) => current.flatMap((item) => {
          if (item.id !== active.assistantId) return [item];
          const content = `${item.content}${pending.delta}`;
          return content
            ? [{
              ...item,
              content,
              firstTokenMs: item.firstTokenMs ?? pending.firstTokenMs ?? undefined,
              streaming: false,
              stopped: true,
            }]
            : [];
        }));
      }
      return true;
    }).catch(() => {
      if (activeRequestRef.current === active) {
        active.cancelling = false;
        active.cancelPromise = null;
        setCancelling(false);
        setComposerError(copy(
          "Could not stop the request. Generation is still running.",
          "无法停止请求，生成仍在继续。",
          "無法停止請求，生成仍在繼續。",
          "リクエストを停止できませんでした。生成はまだ続いています。",
        ));
      }
      return false;
    });
    active.cancelPromise = cancellation;
    return cancellation;
  };

  useEffect(() => {
    if (open) return;
    void (async () => {
      if (!(await cancelActiveRequest(false))) return;
      setItems([]);
      setDraft("");
      setComposerError(null);
      setSending(false);
      setCancelling(false);
    })();
  }, [open]);

  useEffect(() => () => {
    const active = activeRequestRef.current;
    if (!active) return;
    active.stopped = true;
    takePendingDelta(active);
    activeRequestRef.current = null;
    void cancelModelTestChat(active.requestId).catch(() => undefined);
  }, []);

  useEffect(() => {
    const transcript = transcriptRef.current;
    if (transcript) transcript.scrollTop = transcript.scrollHeight;
  }, [items]);

  const clearConversation = async () => {
    if (!(await cancelActiveRequest(false))) return;
    setItems([]);
    setDraft("");
    setComposerError(null);
    focusComposer();
  };

  const stop = async () => {
    if (!(await cancelActiveRequest(true))) return;
    focusComposer();
  };

  const closeConsole = async () => {
    if (!(await cancelActiveRequest(false))) return;
    setItems([]);
    setDraft("");
    setComposerError(null);
    setSending(false);
    setCancelling(false);
    onOpenChange(false);
  };

  const send = async () => {
    const prompt = draft.trim();
    if (!prompt || sending) return;

    const requestMessages = buildModelTestRequestMessages(items, prompt);
    if (requestMessages.error) {
      setComposerError(copy(
        "A message cannot exceed 16,000 UTF-8 bytes.",
        "消息不能超过 16,000 个 UTF-8 字节。",
        "訊息不能超過 16,000 個 UTF-8 位元組。",
        "メッセージは 16,000 UTF-8 バイトを超えることはできません。",
      ));
      return;
    }
    setComposerError(null);
    const requestId = `model-test-${Date.now()}-${nextRequestId.current++}`;
    const userItem: TranscriptItem = {
      id: nextId.current++,
      turnId: requestId,
      role: "user",
      content: prompt,
    };
    const assistantId = nextId.current++;
    const assistantItem: TranscriptItem = {
      id: assistantId,
      turnId: requestId,
      role: "assistant",
      content: "",
      streaming: true,
    };
    const request: ActiveRequest = {
      requestId,
      assistantId,
      stopped: false,
      cancelling: false,
      cancelPromise: null,
      flushTimer: null,
      pendingDelta: "",
      pendingFirstTokenMs: null,
    };
    activeRequestRef.current = request;
    setItems((current) => trimModelTestTranscript([...current, userItem, assistantItem]));
    setDraft("");
    setSending(true);

    try {
      const reply = await testModelChatStream(
        requestMessages.messages,
        request.requestId,
        (event) => {
          if (activeRequestRef.current !== request || request.stopped) return;
          request.pendingDelta += event.delta;
          request.pendingFirstTokenMs ??= event.first_token_ms;
          if (request.flushTimer != null) return;
          request.flushTimer = window.setTimeout(() => {
            const pending = takePendingDelta(request);
            if (activeRequestRef.current !== request || request.stopped || !pending.delta) return;
            setItems((current) => current.map((item) => item.id === assistantId
              ? {
                ...item,
                content: `${item.content}${pending.delta}`,
                firstTokenMs: item.firstTokenMs ?? pending.firstTokenMs ?? undefined,
              }
              : item));
          }, 32);
        },
      );
      if (request.cancelling && request.cancelPromise) {
        const cancelled = await request.cancelPromise;
        if (cancelled) return;
      }
      if (activeRequestRef.current !== request || request.stopped) return;
      takePendingDelta(request);
      setItems((current) => current.map((item) => item.id === assistantId
        ? {
          ...item,
          content: reply.content,
          firstTokenMs: reply.first_token_ms,
          latencyMs: reply.latency_ms,
          streaming: false,
        }
        : item));
    } catch (error) {
      if (request.cancelling && request.cancelPromise) {
        const cancelled = await request.cancelPromise;
        if (cancelled) return;
      }
      if (activeRequestRef.current !== request || request.stopped) return;
      const pending = takePendingDelta(request);
      setComposerError(null);
      setItems((current) => current.map((item) => item.id === assistantId
        ? {
          ...item,
          content: `${item.content}${pending.delta}`,
          firstTokenMs: item.firstTokenMs ?? pending.firstTokenMs ?? undefined,
          streaming: false,
          errorMessage: errorMessage(error),
        }
        : item));
      setDraft(prompt);
    } finally {
      if (activeRequestRef.current === request) {
        takePendingDelta(request);
        activeRequestRef.current = null;
        setSending(false);
        setCancelling(false);
        focusComposer();
      }
    }
  };

  const onComposerKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key !== "Enter" || event.shiftKey || event.nativeEvent.isComposing) return;
    event.preventDefault();
    void send();
  };

  return (
    <Dialog open={open} onOpenChange={(nextOpen) => {
      if (nextOpen) onOpenChange(true);
      else void closeConsole();
    }}>
      <DialogContent
        className="model-test-dialog"
        onOpenAutoFocus={(event) => {
          event.preventDefault();
          focusComposer();
        }}
      >
        <DialogHeader className="model-test-header">
          <div className="model-test-heading-copy">
            <span className="model-test-mark"><MessageSquareText aria-hidden="true" /></span>
            <div>
              <DialogTitle>{copy("Test model", "测试模型", "測試模型", "モデルをテスト")}</DialogTitle>
              <DialogDescription>{copy(
                "Verify the current global routing configuration before connecting an Agent.",
                "接入 Agent 前，先验证当前全局路由配置。",
                "接入 Agent 前，先驗證目前的全域路由配置。",
                "Agent を接続する前に、現在のグローバルルーティング設定を確認します。",
              )}</DialogDescription>
            </div>
          </div>
          <div
            className="model-test-route-status"
            aria-label={copy(
              `${routeStateLabel}: ${routeModeLabel}`,
              `${routeStateLabel}：${routeModeLabel}`,
              `${routeStateLabel}：${routeModeLabel}`,
              `${routeStateLabel}：${routeModeLabel}`,
            )}
          >
            <Route aria-hidden="true" />
            <span>
              <small>{routeStateLabel}</small>
              <strong>{routeModeLabel}</strong>
            </span>
          </div>
        </DialogHeader>

        <div ref={transcriptRef} className="model-test-transcript" aria-live="polite">
          {items.length === 0 ? (
            <div className="model-test-empty">
              <span><Bot aria-hidden="true" /></span>
              <h3>{copy("Start with a real prompt", "发一条真实消息", "傳送一則真實訊息", "実際のメッセージを送信")}</h3>
              <p>{copy(
                "Each send makes one real model request and may count toward Provider usage.",
                "每次发送都会产生一次真实的模型请求，可能计入供应商用量。",
                "每次傳送都會產生一次真實的模型請求，可能計入供應商用量。",
                "送信するたびに実際のモデルリクエストが発生し、プロバイダーの使用量に計上される場合があります。",
              )}</p>
            </div>
          ) : (
            <ol className="model-test-messages">
              {items.map((item) => (
                <li key={item.id} className={`model-test-message ${item.role}${item.errorMessage ? " error" : ""}${item.streaming ? " streaming" : ""}${item.stopped ? " stopped" : ""}`}>
                  <div className="model-test-message-meta">
                    <span>{item.role === "user"
                      ? copy("You", "你", "你", "あなた")
                      : copy("Global route", "全局路由", "全域路由", "グローバルルート")}</span>
                    {item.stopped ? (
                      <small>{copy("Stopped", "已停止", "已停止", "停止済み")}</small>
                    ) : item.firstTokenMs != null && item.latencyMs != null ? (
                      <small>{copy(
                        `First text ${item.firstTokenMs.toLocaleString()} ms · Total ${item.latencyMs.toLocaleString()} ms`,
                        `首字 ${item.firstTokenMs.toLocaleString()} ms · 总计 ${item.latencyMs.toLocaleString()} ms`,
                        `首字 ${item.firstTokenMs.toLocaleString()} ms · 總計 ${item.latencyMs.toLocaleString()} ms`,
                        `初回文字 ${item.firstTokenMs.toLocaleString()} ms · 合計 ${item.latencyMs.toLocaleString()} ms`,
                      )}</small>
                    ) : null}
                  </div>
                  {item.content ? (
                    <p>
                      {item.content}
                      {item.streaming && <span className="model-test-stream-caret" aria-hidden="true" />}
                    </p>
                  ) : item.streaming ? (
                    <p className="model-test-waiting"><LoaderCircle aria-hidden="true" /></p>
                  ) : null}
                  {item.errorMessage && (
                    <small className="model-test-message-error" role="alert">{item.errorMessage}</small>
                  )}
                </li>
              ))}
            </ol>
          )}
        </div>

        <div className="model-test-composer-shell">
          <textarea
            ref={composerRef}
            className="model-test-composer"
            aria-label={copy("Message", "消息", "訊息", "メッセージ")}
            placeholder={copy("Message the global route…", "向全局路由发送消息…", "向全域路由傳送訊息…", "グローバルルートにメッセージを送信…")}
            value={draft}
            maxLength={16_000}
            rows={2}
            onChange={(event) => {
              setDraft(event.target.value);
              setComposerError(null);
            }}
            onKeyDown={onComposerKeyDown}
          />
          {composerError && (
            <small className="model-test-composer-error" role="alert">{composerError}</small>
          )}
          <div className="model-test-composer-actions">
            <Button
              type="button"
              variant="ghost"
              size="icon-sm"
              aria-label={copy("Clear conversation", "清空对话", "清空對話", "会話を消去")}
              disabled={items.length === 0 || cancelling}
              onClick={() => void clearConversation()}
            >
              <Trash2 aria-hidden="true" />
            </Button>
            <span>{copy("Enter to send · Shift+Enter for a new line", "Enter 发送 · Shift+Enter 换行", "Enter 傳送 · Shift+Enter 換行", "Enter で送信 · Shift+Enter で改行")}</span>
            <Button
              type="button"
              size="icon"
              className="model-test-send"
              aria-label={cancelling
                ? copy("Stopping generation", "正在停止生成", "正在停止生成", "生成を停止しています")
                : sending
                  ? copy("Stop generating", "停止生成", "停止生成", "生成を停止")
                : copy("Send message", "发送消息", "傳送訊息", "メッセージを送信")}
              disabled={cancelling || (!sending && !draft.trim())}
              onClick={sending ? () => void stop() : () => void send()}
            >
              {sending ? <Square className="model-test-stop-icon" aria-hidden="true" /> : <SendHorizontal aria-hidden="true" />}
            </Button>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}
