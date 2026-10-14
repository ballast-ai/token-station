import type { PlanConnectionKind } from "../planConnections";
import { planConnectionTitle } from "../planConnections";
import { useLocalizedCopy } from "./LanguageProvider";
import ChatGPTPlanPanel from "./ChatGPTPlanPanel";
import LocalCodexPanel from "./LocalCodexPanel";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "./ui/dialog";

export default function PlanConnectionDialog({ kind, onClose, onChanged, onConnected }: {
  kind: PlanConnectionKind | null;
  onClose: () => void;
  onChanged: () => Promise<void>;
  onConnected: () => void;
}) {
  const { copy } = useLocalizedCopy();
  return <Dialog open={kind !== null} onOpenChange={open => { if (!open) onClose(); }}>
    {kind && <DialogContent className="plan-connection-dialog" closeLabel={copy("Close", "关闭", "關閉", "閉じる")}
      onCloseAutoFocus={event => {
        event.preventDefault();
        const trigger = document.querySelector<HTMLButtonElement>(".providers-add-button")
          ?? document.querySelector<HTMLButtonElement>("[data-model-entry-trigger]");
        trigger?.focus();
      }}>
      <DialogHeader><DialogTitle>{planConnectionTitle(kind, copy)}</DialogTitle><DialogDescription>{kind === "codex"
        ? copy("Reuse your existing login without browser authorization.", "复用已有登录，无需浏览器重新授权。", "重用現有登入，無需瀏覽器重新授權。", "既存のサインインを再利用します。ブラウザー認証は不要です。")
        : copy("Authorize your ChatGPT account in your browser.", "通过浏览器完成 ChatGPT 账号授权。", "透過瀏覽器完成 ChatGPT 帳號授權。", "ブラウザーで ChatGPT アカウントを認証してください。")}</DialogDescription></DialogHeader>
      <div className="plan-connection-dialog-body">{kind === "codex"
        ? <LocalCodexPanel onChanged={onChanged} onConnected={onConnected} />
        : <ChatGPTPlanPanel onChanged={onChanged} onConnected={onConnected} />}</div>
    </DialogContent>}
  </Dialog>;
}
