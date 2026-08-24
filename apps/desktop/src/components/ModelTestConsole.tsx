import { useEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { Bot, Check, ChevronLeft, ChevronRight, ChevronsUpDown, LoaderCircle, MessageSquareText, SendHorizontal, Square, Trash2 } from "lucide-react";
import type { ModelTestMessage, ProviderView, TierView } from "../api";
import { cancelModelTestChat, testModelChatStream } from "../api";
import { ProviderIcon } from "../brandIcons";
import { useLocalizedCopy } from "./LanguageProvider";
import { Button } from "./ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "./ui/dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "./ui/dropdown-menu";

interface ModelTestConsoleProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  providers: ProviderView[];
  initialTarget: TierView | null;
}

type TranscriptItem = ModelTestMessage & {
  id: number;
  latencyMs?: number;
  firstTokenMs?: number;
  streaming?: boolean;
  stopped?: boolean;
  errorMessage?: string;
};

type ActiveRequest = {
  requestId: string;
  assistantId: number;
  stopped: boolean;
};

const targetKey = (upstream: string, model: string) => `${upstream}\u0000${model}`;

function errorMessage(error: unknown) {
  if (error instanceof Error && error.message.trim()) return error.message;
  if (typeof error === "string" && error.trim()) return error;
  return "Model request failed";
}

export default function ModelTestConsole({
  open,
  onOpenChange,
  providers,
  initialTarget,
}: ModelTestConsoleProps) {
  const { copy } = useLocalizedCopy();
  const composerRef = useRef<HTMLTextAreaElement>(null);
  const transcriptRef = useRef<HTMLDivElement>(null);
  const nextId = useRef(1);
  const nextRequestId = useRef(1);
  const activeRequestRef = useRef<ActiveRequest | null>(null);
  const targets = useMemo(() => providers.flatMap((provider) => provider.models.map((model) => ({
    provider,
    model,
    key: targetKey(provider.name, model),
  }))), [providers]);
  const preferredKey = initialTarget?.upstream && initialTarget.model
    ? targetKey(initialTarget.upstream, initialTarget.model)
    : null;
  const defaultKey = targets.some((target) => target.key === preferredKey)
    ? preferredKey!
    : (targets[0]?.key ?? "");
  const [selectedKey, setSelectedKey] = useState(defaultKey);
  const [items, setItems] = useState<TranscriptItem[]>([]);
  const [draft, setDraft] = useState("");
  const [sending, setSending] = useState(false);
  const [targetMenuOpen, setTargetMenuOpen] = useState(false);
  const [pickerProviderName, setPickerProviderName] = useState<string | null>(null);
  const focusComposer = (delay = 60) => window.setTimeout(() => composerRef.current?.focus(), delay);

  const cancelActiveRequest = (preservePartial: boolean) => {
    const active = activeRequestRef.current;
    if (!active) return;
    active.stopped = true;
    activeRequestRef.current = null;
    void cancelModelTestChat(active.requestId);
    setSending(false);
    if (preservePartial) {
      setItems((current) => current.flatMap((item) => {
        if (item.id !== active.assistantId) return [item];
        return item.content
          ? [{ ...item, streaming: false, stopped: true }]
          : [];
      }));
    }
  };

  useEffect(() => {
    if (!targets.some((target) => target.key === selectedKey)) setSelectedKey(defaultKey);
  }, [defaultKey, selectedKey, targets]);

  useEffect(() => {
    if (open) return;
    cancelActiveRequest(false);
    setItems([]);
    setDraft("");
    setSending(false);
    setSelectedKey(defaultKey);
    setTargetMenuOpen(false);
    setPickerProviderName(null);
  }, [defaultKey, open]);

  useEffect(() => () => {
    const active = activeRequestRef.current;
    if (!active) return;
    active.stopped = true;
    activeRequestRef.current = null;
    void cancelModelTestChat(active.requestId);
  }, []);

  useEffect(() => {
    const transcript = transcriptRef.current;
    if (transcript) transcript.scrollTop = transcript.scrollHeight;
  }, [items]);

  const selectedTarget = targets.find((target) => target.key === selectedKey) ?? targets[0];
  const selectableProviders = providers.filter((provider) => provider.models.length > 0);
  const pickerProvider = selectableProviders.find((provider) => provider.name === pickerProviderName);

  const clearConversation = () => {
    cancelActiveRequest(false);
    setItems([]);
    setDraft("");
    focusComposer();
  };

  const changeTarget = (key: string) => {
    cancelActiveRequest(false);
    setSelectedKey(key);
    setItems([]);
    setDraft("");
    setTargetMenuOpen(false);
    setPickerProviderName(null);
    focusComposer(220);
  };

  const stop = () => {
    cancelActiveRequest(true);
    focusComposer();
  };

  const send = async () => {
    const prompt = draft.trim();
    if (!selectedTarget || !prompt || sending) return;

    const history: ModelTestMessage[] = items
      .filter((item) => !item.errorMessage && !item.stopped && !item.streaming)
      .map(({ role, content }) => ({ role, content }));
    const requestMessages = [...history.slice(-19), { role: "user" as const, content: prompt }];
    const userItem: TranscriptItem = { id: nextId.current++, role: "user", content: prompt };
    const assistantId = nextId.current++;
    const assistantItem: TranscriptItem = {
      id: assistantId,
      role: "assistant",
      content: "",
      streaming: true,
    };
    const request: ActiveRequest = {
      requestId: `model-test-${Date.now()}-${nextRequestId.current++}`,
      assistantId,
      stopped: false,
    };
    activeRequestRef.current = request;
    setItems((current) => [...current, userItem, assistantItem]);
    setDraft("");
    setSending(true);

    try {
      const reply = await testModelChatStream(
        selectedTarget.provider.name,
        selectedTarget.model,
        requestMessages,
        request.requestId,
        (event) => {
          if (activeRequestRef.current !== request || request.stopped) return;
          setItems((current) => current.map((item) => item.id === assistantId
            ? {
              ...item,
              content: `${item.content}${event.delta}`,
              firstTokenMs: item.firstTokenMs ?? event.first_token_ms ?? undefined,
            }
            : item));
        },
      );
      if (activeRequestRef.current !== request || request.stopped) return;
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
      if (activeRequestRef.current !== request || request.stopped) return;
      setItems((current) => current.map((item) => item.id === assistantId
        ? { ...item, streaming: false, errorMessage: errorMessage(error) }
        : item));
      setDraft(prompt);
    } finally {
      if (activeRequestRef.current === request) {
        activeRequestRef.current = null;
        setSending(false);
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
      if (!nextOpen) cancelActiveRequest(false);
      onOpenChange(nextOpen);
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
                "Have a short conversation before using this model in an Agent.",
                "接入 Agent 前，先用真实对话确认模型响应。",
                "接入 Agent 前，先用真實對話確認模型回應。",
                "Agent で使う前に、実際の会話でモデルの応答を確認します。",
              )}</DialogDescription>
            </div>
          </div>
          {selectedTarget && (
            <DropdownMenu open={targetMenuOpen} onOpenChange={(nextOpen) => {
              setTargetMenuOpen(nextOpen);
              if (nextOpen) setPickerProviderName(null);
            }}>
              <DropdownMenuTrigger asChild>
                <button
                  type="button"
                  className="model-test-target-trigger"
                  aria-label={copy(
                    `Choose model. Current: ${selectedTarget.model}, Provider: ${selectedTarget.provider.name}`,
                    `选择模型，当前：${selectedTarget.model}，供应商：${selectedTarget.provider.name}`,
                    `選擇模型，目前：${selectedTarget.model}，供應商：${selectedTarget.provider.name}`,
                    `モデルを選択。現在：${selectedTarget.model}、プロバイダー：${selectedTarget.provider.name}`,
                  )}
                >
                  <ProviderIcon id={selectedTarget.provider.brand_id} label={selectedTarget.provider.name} size={20} />
                  <span className="model-test-target-copy">
                    <strong>{selectedTarget.model}</strong>
                    <small>{selectedTarget.provider.name}</small>
                  </span>
                  <ChevronsUpDown aria-hidden="true" />
                </button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end" sideOffset={8} className="model-test-target-menu">
                {pickerProvider ? (
                  <>
                    <DropdownMenuItem
                      className="model-test-picker-back"
                      aria-label={copy("Back to Providers", "返回供应商", "返回供應商", "プロバイダーに戻る")}
                      onSelect={(event) => {
                        event.preventDefault();
                        setPickerProviderName(null);
                      }}
                    >
                      <ChevronLeft aria-hidden="true" />
                      <ProviderIcon id={pickerProvider.brand_id} label={pickerProvider.name} size={20} />
                      <span className="model-test-picker-heading">
                        <small>{copy("Choose model", "选择模型", "選擇模型", "モデルを選択")}</small>
                        <strong>{pickerProvider.name}</strong>
                      </span>
                    </DropdownMenuItem>
                    <DropdownMenuSeparator />
                    {pickerProvider.models.map((model) => {
                      const key = targetKey(pickerProvider.name, model);
                      return (
                        <DropdownMenuItem key={key} onSelect={() => changeTarget(key)}>
                          <span className="model-test-model-name">{model}</span>
                          {selectedKey === key && <Check aria-hidden="true" />}
                        </DropdownMenuItem>
                      );
                    })}
                  </>
                ) : (
                  <>
                    <DropdownMenuLabel>{copy("Choose Provider", "选择供应商", "選擇供應商", "プロバイダーを選択")}</DropdownMenuLabel>
                    {selectableProviders.map((provider) => (
                      <DropdownMenuItem
                        key={provider.name}
                        onSelect={(event) => {
                          event.preventDefault();
                          setPickerProviderName(provider.name);
                        }}
                      >
                        <ProviderIcon id={provider.brand_id} label={provider.name} size={20} />
                        <strong>{provider.name}</strong>
                        {provider.name === selectedTarget.provider.name
                          ? <Check aria-hidden="true" />
                          : <ChevronRight aria-hidden="true" />}
                      </DropdownMenuItem>
                    ))}
                  </>
                )}
              </DropdownMenuContent>
            </DropdownMenu>
          )}
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
                      : selectedTarget?.model}</span>
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
            placeholder={copy("Message the model…", "给模型发消息…", "傳訊息給模型…", "モデルにメッセージを送信…")}
            value={draft}
            maxLength={16_000}
            rows={2}
            disabled={!selectedTarget}
            onChange={(event) => setDraft(event.target.value)}
            onKeyDown={onComposerKeyDown}
          />
          <div className="model-test-composer-actions">
            <Button
              type="button"
              variant="ghost"
              size="icon-sm"
              aria-label={copy("Clear conversation", "清空对话", "清空對話", "会話を消去")}
              disabled={items.length === 0}
              onClick={clearConversation}
            >
              <Trash2 aria-hidden="true" />
            </Button>
            <span>{copy("Enter to send · Shift+Enter for a new line", "Enter 发送 · Shift+Enter 换行", "Enter 傳送 · Shift+Enter 換行", "Enter で送信 · Shift+Enter で改行")}</span>
            <Button
              type="button"
              size="icon"
              className="model-test-send"
              aria-label={sending
                ? copy("Stop generating", "停止生成", "停止生成", "生成を停止")
                : copy("Send message", "发送消息", "傳送訊息", "メッセージを送信")}
              disabled={!sending && (!draft.trim() || !selectedTarget)}
              onClick={sending ? stop : () => void send()}
            >
              {sending ? <Square className="model-test-stop-icon" aria-hidden="true" /> : <SendHorizontal aria-hidden="true" />}
            </Button>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}
