import { useState, type ReactNode } from "react";
import { ExternalLink, Monitor, RefreshCw } from "lucide-react";
import { ProviderIcon } from "../brandIcons";
import type { PlanConnectionKind } from "../planConnections";
import { useLocalizedCopy } from "./LanguageProvider";
import { Button } from "./ui/button";
import { Badge } from "./ui/badge";
import { Separator } from "./ui/separator";
import {
  AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent,
  AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle, AlertDialogTrigger,
} from "./ui/alert-dialog";

interface PlanConnectionContentProps {
  kind: PlanConnectionKind;
  accountId: string;
  label: string;
  email: string | null;
  ready: boolean;
  busy: boolean;
  managed: boolean;
  models: Array<{ id: string; name: string }>;
  catalogLoaded?: boolean;
  recovery?: ReactNode;
  connect?: ReactNode;
  onRefresh: () => Promise<void>;
  onUsage: () => Promise<void>;
  onDisconnect: () => Promise<void>;
  canDisconnect?: boolean;
  details?: ReactNode;
}

/** Shared account presentation. Each transport keeps its own authentication controller. */
export default function PlanConnectionContent(props: PlanConnectionContentProps) {
  const { copy } = useLocalizedCopy();
  const [confirmOpen, setConfirmOpen] = useState(false);
  const local = props.kind === "codex";
  const disconnectLabel = local
    ? copy("Disconnect reference", "断开引用", "中斷參照", "参照を解除")
    : copy("Disconnect connection", "断开连接", "中斷連線", "接続を解除");

  return <article className="plan-connection-account">
    <div className="plan-account-summary">
      <span className="plan-account-icon" aria-hidden="true">{local ? <Monitor /> : <ProviderIcon id="openai" label="ChatGPT" size={30} />}</span>
      <div className="plan-account-identity"><strong>{props.label || props.email || props.accountId}</strong>
        {props.email && props.email !== props.label && <span>{props.email}</span>}
        <small>{local
          ? copy("Reuse local Codex login", "复用本机 Codex 登录", "重用本機 Codex 登入", "ローカル Codex のサインインを再利用")
          : copy("Connected through browser authorization", "通过浏览器授权连接", "透過瀏覽器授權連接", "ブラウザー認証で接続")}</small>
      </div>
      {props.ready && <Badge variant="secondary" className="plan-account-status ready">{props.managed ? copy("Connected", "已连接", "已連線", "接続済み") : copy("Ready", "可用", "可用", "準備完了")}</Badge>}
      {!props.ready && <Badge variant="secondary" className="plan-account-status unavailable">{copy("Unavailable", "不可用", "無法使用", "利用不可")}</Badge>}
    </div>
    {props.recovery}
    {props.connect}
    {props.managed && <><div className="plan-account-usage"><div><strong>{copy("Plan usage", "套餐用量", "方案用量", "プラン使用量")}</strong>
      <p>{copy("Shared with ChatGPT. Requests stop when allowance runs out.", "与 ChatGPT 共享额度，用尽后停止请求。", "與 ChatGPT 共用額度，用盡後停止請求。", "ChatGPT と利用枠を共有します。枠を使い切るとリクエストが停止します。")}</p></div>
      <Button type="button" variant="link" size="sm" disabled={props.busy} onClick={() => void props.onUsage()}>{copy("View usage", "查看用量", "檢視用量", "使用量を表示")}<ExternalLink data-icon="inline-end" aria-hidden="true" /></Button>
    </div>
    <section className="plan-account-models"><div className="plan-account-model-heading"><h3>{!props.catalogLoaded
      ? copy("Connected models", "已接入模型", "已接入模型", "接続済みモデル")
      : copy("Available models", "可用模型", "可用模型", "利用可能なモデル")}</h3>
      <Badge variant="secondary" className="provider-model-count">{props.models.length}</Badge>
      <Button type="button" variant="secondary" size="sm" disabled={props.busy || !props.ready} onClick={() => void props.onRefresh()}><RefreshCw data-icon="inline-start" aria-hidden="true" />{copy("Refresh models", "刷新模型", "重新整理模型", "モデルを更新")}</Button>
    </div>
    {props.models.length > 0 ? <ul className="plan-account-model-grid" aria-label={local ? copy("Available Codex models", "可用 Codex 模型", "可用 Codex 模型", "利用可能な Codex モデル") : copy("Available models", "可用模型", "可用模型", "利用可能なモデル")}>
      {props.models.map(model => <li key={model.id} title={model.id}><strong>{model.name}</strong>{model.name !== model.id && <code>{model.id}</code>}</li>)}
    </ul> : <p className="plan-account-note">{props.catalogLoaded
      ? copy("No available models returned.", "没有返回可用模型。", "沒有傳回可用模型。", "利用可能なモデルが返されませんでした。")
      : copy("No models loaded. Refresh to check the account catalog.", "尚未加载模型，请刷新以查看账号目录。", "尚未載入模型，請重新整理以檢視帳號目錄。", "モデルが未読み込みです。更新してアカウントの一覧を確認してください。")}</p>}
    {props.managed && <p className="plan-account-note">{copy("These models share this account connection.", "这些模型共用当前账号的连接。", "這些模型共用目前帳號的連線。", "これらのモデルはこのアカウントの接続を共有します。")}</p>}
    </section></>}
    <details className="plan-account-details"><summary>{copy("Connection details and usage limits", "连接详情与使用限制", "連線詳情與使用限制", "接続情報と利用制限")}</summary>
      <p>{copy("Connection", "连接", "連線", "接続")}: <code>{props.accountId}</code></p>
      <p>{copy("No API credit or dollar cost estimate. Temperature and output limits are unsupported.", "不使用 API 余额，也不估算美元费用。暂不支持温度和输出上限。", "不使用 API 餘額，也不估算美元費用。暫不支援溫度和輸出上限。", "API クレジットやドル費用の推定はありません。温度と出力上限は未対応です。")}</p>
      {props.details}
    </details>
    {props.managed && <><Separator />
    <div className="plan-account-disconnect"><p>{local
      ? copy("Disconnecting here does not sign Codex out. This connection's models become unavailable.", "在这里断开不会退出 Codex 登录，此连接下的模型将不可用。", "在此中斷不會登出 Codex，此連線下的模型將無法使用。", "接続を解除しても Codex からはサインアウトしません。モデルは利用できなくなります。")
      : copy("Disconnecting makes this account's models unavailable.", "断开后，这个账号下的模型不可用。", "中斷後，此帳號下的模型無法使用。", "接続を解除すると、このアカウントのモデルは利用できなくなります。")}</p>
      <AlertDialog open={confirmOpen} onOpenChange={setConfirmOpen}>
        <AlertDialogTrigger asChild><Button type="button" variant="ghost" size="sm" disabled={props.busy || props.canDisconnect === false}>{disconnectLabel}</Button></AlertDialogTrigger>
        <AlertDialogContent><AlertDialogHeader><AlertDialogTitle>{local
          ? copy("Disconnect the local Codex reference?", "断开本机 Codex 引用？", "中斷本機 Codex 參照？", "Codex の参照を解除しますか？")
          : copy("Disconnect the ChatGPT connection?", "断开 ChatGPT 连接？", "中斷 ChatGPT 連線？", "ChatGPT の接続を解除しますか？")}</AlertDialogTitle>
          <AlertDialogDescription>{local
            ? copy("All models that use this reference become unavailable. Codex remains signed in.", "使用此引用的全部模型将不可用，Codex 仍保持登录。", "使用此參照的全部模型將無法使用，Codex 仍保持登入。", "この参照を使うすべてのモデルが利用不可になります。Codex はサインインしたままです。")
            : copy("All models that use this registration become unavailable. You can reconnect the account later.", "使用此授权的全部模型将不可用，之后可以重新连接账号。", "使用此授權的全部模型將無法使用，之後可以重新連接帳號。", "この認証を使うすべてのモデルが利用不可になります。後で再接続できます。")}</AlertDialogDescription></AlertDialogHeader>
          <AlertDialogFooter><AlertDialogCancel>{copy("Cancel", "取消", "取消", "キャンセル")}</AlertDialogCancel><AlertDialogAction variant="destructive" onClick={() => void props.onDisconnect()}>{disconnectLabel}</AlertDialogAction></AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div></>}
  </article>;
}
