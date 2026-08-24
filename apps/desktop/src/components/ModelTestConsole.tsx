import { useEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { Bot, LoaderCircle, MessageSquareText, SendHorizontal, Trash2 } from "lucide-react";
import type { ModelTestMessage, ProviderView, TierView } from "../api";
import { testModelChat } from "../api";
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
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectLabel,
  SelectTrigger,
} from "./ui/select";

interface ModelTestConsoleProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  providers: ProviderView[];
  initialTarget: TierView | null;
}

type TranscriptItem = ModelTestMessage & {
  id: number;
  latencyMs?: number;
  error?: boolean;
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
  const nextId = useRef(1);
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
  const focusComposer = (delay = 60) => window.setTimeout(() => composerRef.current?.focus(), delay);

  useEffect(() => {
    if (!targets.some((target) => target.key === selectedKey)) setSelectedKey(defaultKey);
  }, [defaultKey, selectedKey, targets]);

  useEffect(() => {
    if (open) return;
    setItems([]);
    setDraft("");
    setSending(false);
    setSelectedKey(defaultKey);
  }, [defaultKey, open]);

  const selectedTarget = targets.find((target) => target.key === selectedKey) ?? targets[0];

  const clearConversation = () => {
    setItems([]);
    setDraft("");
    focusComposer();
  };

  const changeTarget = (key: string) => {
    setSelectedKey(key);
    setItems([]);
    setDraft("");
    focusComposer(220);
  };

  const send = async () => {
    const prompt = draft.trim();
    if (!selectedTarget || !prompt || sending) return;

    const history: ModelTestMessage[] = items
      .filter((item) => !item.error)
      .map(({ role, content }) => ({ role, content }));
    const requestMessages = [...history.slice(-19), { role: "user" as const, content: prompt }];
    const userItem: TranscriptItem = { id: nextId.current++, role: "user", content: prompt };
    setItems((current) => [...current, userItem]);
    setDraft("");
    setSending(true);

    try {
      const reply = await testModelChat(selectedTarget.provider.name, selectedTarget.model, requestMessages);
      setItems((current) => [...current, {
        id: nextId.current++,
        role: "assistant",
        content: reply.content,
        latencyMs: reply.latency_ms,
      }]);
    } catch (error) {
      setItems((current) => [...current, {
        id: nextId.current++,
        role: "assistant",
        content: errorMessage(error),
        error: true,
      }]);
      setDraft(prompt);
    } finally {
      setSending(false);
      focusComposer();
    }
  };

  const onComposerKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key !== "Enter" || event.shiftKey || event.nativeEvent.isComposing) return;
    event.preventDefault();
    void send();
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
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
            <Select value={selectedTarget.key} onValueChange={changeTarget} disabled={sending}>
              <SelectTrigger className="model-test-target" aria-label={copy("Test model", "测试模型", "測試模型", "テストモデル")}>
                <ProviderIcon id={selectedTarget.provider.brand_id} label={selectedTarget.provider.name} size={20} />
                <span className="model-test-target-copy">
                  <strong>{selectedTarget.model}</strong>
                  <small>{selectedTarget.provider.name}</small>
                </span>
              </SelectTrigger>
              <SelectContent position="popper" align="end" className="model-test-target-menu">
                {providers.map((provider) => (
                  <SelectGroup key={provider.name}>
                    <SelectLabel>{provider.name}</SelectLabel>
                    {provider.models.map((model) => (
                      <SelectItem key={targetKey(provider.name, model)} value={targetKey(provider.name, model)}>
                        <ProviderIcon id={provider.brand_id} label={provider.name} size={18} />
                        <span className="model-test-option-copy">
                          <strong>{model}</strong>
                          <small>{provider.name}</small>
                        </span>
                      </SelectItem>
                    ))}
                  </SelectGroup>
                ))}
              </SelectContent>
            </Select>
          )}
        </DialogHeader>

        <div className="model-test-transcript" aria-live="polite">
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
                <li key={item.id} className={`model-test-message ${item.role}${item.error ? " error" : ""}`}>
                  <div className="model-test-message-meta">
                    <span>{item.role === "user"
                      ? copy("You", "你", "你", "あなた")
                      : selectedTarget?.model}</span>
                    {item.latencyMs != null && <small>{item.latencyMs.toLocaleString()} ms</small>}
                  </div>
                  <p role={item.error ? "alert" : undefined}>{item.content}</p>
                </li>
              ))}
              {sending && (
                <li className="model-test-message assistant pending" aria-label={copy("Model is replying", "模型正在回复", "模型正在回覆", "モデルが応答中")}>
                  <div className="model-test-message-meta"><span>{selectedTarget?.model}</span></div>
                  <p><LoaderCircle aria-hidden="true" /></p>
                </li>
              )}
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
              disabled={items.length === 0 || sending}
              onClick={clearConversation}
            >
              <Trash2 aria-hidden="true" />
            </Button>
            <span>{copy("Enter to send · Shift+Enter for a new line", "Enter 发送 · Shift+Enter 换行", "Enter 傳送 · Shift+Enter 換行", "Enter で送信 · Shift+Enter で改行")}</span>
            <Button
              type="button"
              size="icon"
              className="model-test-send"
              aria-label={copy("Send message", "发送消息", "傳送訊息", "メッセージを送信")}
              disabled={!draft.trim() || sending || !selectedTarget}
              onClick={() => void send()}
            >
              {sending ? <LoaderCircle className="model-test-spinner" aria-hidden="true" /> : <SendHorizontal aria-hidden="true" />}
            </Button>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}
