import { useLayoutEffect, useRef, useState } from "react";
import { ArrowLeft } from "lucide-react";
import { Claude, OpenAI, GithubCopilot, Gemini, Antigravity } from "@lobehub/icons";
import { SUBSCRIPTION_CHANNELS, SUBSCRIPTION_LABELS, type SubscriptionChannel } from "../subscriptions";
import { useLocalizedCopy } from "./LanguageProvider";
import LocalCodexPanel from "./LocalCodexPanel";
import ChatGPTPlanPanel from "./ChatGPTPlanPanel";
import LocalSubscriptionPanel from "./LocalSubscriptionPanel";
import { Button } from "./ui/button";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "./ui/dialog";

export function SubscriptionIcon({ channel }: { channel: SubscriptionChannel }) {
  if (channel === "claude") return <Claude.Color size={28} />;
  if (channel === "copilot") return <GithubCopilot size={28} />;
  if (channel === "gemini-cli") return <Gemini.Color size={28} />;
  if (channel === "antigravity") return <Antigravity.Color size={28} />;
  return <OpenAI size={28} />;
}

export default function SubscriptionsDialog({ open, onClose, onChanged, onConnected }: {
  open: boolean;
  onClose: () => void;
  onChanged: () => Promise<void>;
  onConnected: () => void;
}) {
  const { copy } = useLocalizedCopy();
  const [channel, setChannel] = useState<SubscriptionChannel | null>(null);
  const returnTo = useRef<SubscriptionChannel | null>(null);
  const buttons = useRef<Partial<Record<SubscriptionChannel, HTMLButtonElement | null>>>({});
  useLayoutEffect(() => {
    if (!open) { setChannel(null); returnTo.current = null; }
    else if (!channel && returnTo.current) {
      buttons.current[returnTo.current]?.focus();
      returnTo.current = null;
    }
  }, [open, channel]);
  return <Dialog open={open} onOpenChange={value => { if (!value) onClose(); }}>
    <DialogContent className="plan-connection-dialog subscriptions-dialog" closeLabel={copy("Close", "关闭", "關閉", "閉じる")}
      onCloseAutoFocus={event => {
        event.preventDefault();
        (document.querySelector<HTMLButtonElement>(".providers-add-button")
          ?? document.querySelector<HTMLButtonElement>("[data-model-entry-trigger]"))?.focus();
      }}>
      <DialogHeader>
        {channel && <Button variant="ghost" size="sm" className="subscription-back" onClick={() => { returnTo.current = channel; setChannel(null); }}>
          <ArrowLeft data-icon="inline-start" aria-hidden="true" />{copy("Back to subscriptions", "返回订阅", "返回訂閱", "サブスクリプションに戻る")}
        </Button>}
        <DialogTitle>{channel ? SUBSCRIPTION_LABELS[channel] : copy("Subscriptions", "订阅", "訂閱", "サブスクリプション")}</DialogTitle>
        <DialogDescription>{channel === "chatgpt-api"
          ? copy("Authorize your ChatGPT account to use its plan through the API.", "通过 ChatGPT 账号授权，使用套餐额度调用 API。", "透過 ChatGPT 帳號授權，使用方案額度呼叫 API。", "ChatGPT アカウントを認証し、プラン枠で API を使用します。")
          : copy("Reuse a login on this computer and connect its available models.", "复用本机已有登录，接入账号可用的模型。", "重用本機現有登入，接入帳號可用的模型。", "この端末のサインインを再利用し、利用可能なモデルを接続します。")}</DialogDescription>
      </DialogHeader>
      <div className="plan-connection-dialog-body">
        {!channel ? <>
          <div className="subscription-channel-grid" role="group" aria-label={copy("Subscription channels", "订阅渠道", "訂閱渠道", "サブスクリプションチャネル")}>
            {SUBSCRIPTION_CHANNELS.map(kind => <button key={kind} type="button" aria-label={SUBSCRIPTION_LABELS[kind]}
              ref={element => { buttons.current[kind] = element; }} onClick={() => setChannel(kind)}>
              <span aria-hidden="true"><SubscriptionIcon channel={kind} /></span><strong>{SUBSCRIPTION_LABELS[kind]}</strong>
            </button>)}
          </div>
          <p className="model-entry-plan-note">{copy("Plan connections share account allowance. ChatGPT API uses account authorization rather than an API key.", "订阅连接共享账号额度。ChatGPT API 使用账号授权，无需 API Key。", "訂閱連線共用帳號額度。ChatGPT API 使用帳號授權，無需 API Key。", "プラン接続はアカウントの利用枠を共有します。ChatGPT API は API キーではなくアカウント認証を使用します。")}</p>
        </> : channel === "chatgpt"
          ? <LocalCodexPanel onChanged={onChanged} onConnected={onConnected} />
          : channel === "chatgpt-api"
            ? <ChatGPTPlanPanel onChanged={onChanged} onConnected={onConnected} />
            : <LocalSubscriptionPanel key={channel} kind={channel} onChanged={onChanged} onConnected={onConnected} />}
      </div>
    </DialogContent>
  </Dialog>;
}
