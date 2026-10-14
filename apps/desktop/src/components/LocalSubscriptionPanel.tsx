import { useEffect, useRef, useState } from "react";
import type { ProviderView } from "../api";
import { connectSubscription, discoverSubscription, disconnectSubscription, getSubscriptionModels, listSubscriptions, type SubscriptionAccount, type SubscriptionModel } from "../localSubscriptionsApi";
import { SUBSCRIPTION_LABELS, type LocalSubscriptionKind } from "../subscriptions";
import { useLocalizedCopy } from "./LanguageProvider";
import { Alert, AlertDescription } from "./ui/alert";
import { Badge } from "./ui/badge";
import { Button } from "./ui/button";

export default function LocalSubscriptionPanel({ kind, provider, onChanged, onConnected }: {
  kind: LocalSubscriptionKind;
  provider?: ProviderView;
  onChanged: () => Promise<void>;
  onConnected?: () => void;
}) {
  const { copy } = useLocalizedCopy();
  const label = SUBSCRIPTION_LABELS[kind];
  const [accounts, setAccounts] = useState<SubscriptionAccount[]>([]);
  const [detected, setDetected] = useState<SubscriptionAccount | null>(null);
  const [catalogs, setCatalogs] = useState<Record<string, SubscriptionModel[]>>({});
  const [busy, setBusy] = useState(true);
  const [error, setError] = useState(false);
  const generation = useRef(0);
  useEffect(() => {
    const current = ++generation.current;
    setBusy(true); setError(false);
    void Promise.all([listSubscriptions(kind), discoverSubscription(kind)])
      .then(([registered, local]) => { if (generation.current === current) { setAccounts(registered); setDetected(local); } })
      .catch(() => { if (generation.current === current) setError(true); })
      .finally(() => { if (generation.current === current) setBusy(false); });
    return () => { generation.current++; };
  }, [kind]);
  const run = async (action: (active: () => boolean) => Promise<void>) => {
    if (busy) return;
    const current = generation.current;
    const active = () => generation.current === current;
    setBusy(true); setError(false);
    try { await action(active); }
    catch { if (active()) setError(true); }
    finally { if (active()) setBusy(false); }
  };
  const stateText = (state: string) => {
    if (state === "ready") return copy("Local login ready", "本机登录可用", "本機登入可用", "ローカルのサインインを利用できます");
    if (state === "account_changed") return copy("The local account changed. Disconnect the old reference and reconnect.", "本机账号已变更，请断开旧引用后重新接入。", "本機帳號已變更，請中斷舊參照後重新接入。", "アカウントが変更されました。古い参照を解除して再接続してください。");
    if (state === "unsupported_storage") return copy("This local login storage is unsupported. Sign in using a supported local client.", "暂不支持当前登录存储方式，请使用受支持的本机客户端登录。", "暫不支援目前登入儲存方式，請使用受支援的本機用戶端登入。", "この保存方式は未対応です。対応するローカルクライアントでサインインしてください。");
    if (state === "invalid_source") return copy(`The local ${label} login cannot be read. Check its credential storage and sign in again if needed.`, `无法读取本机 ${label} 登录，请检查凭据存储方式，必要时重新登录。`, `無法讀取本機 ${label} 登入，請檢查憑證儲存方式，必要時重新登入。`, `ローカルの ${label} 認証情報を読み取れません。保存方式を確認し、必要なら再サインインしてください。`);
    if (state === "expired") return copy(`Renew the login in ${label}, then retry.`, `请在 ${label} 中更新登录后重试。`, `請在 ${label} 中更新登入後重試。`, `${label} でサインインを更新して再試行してください。`);
    if (state === "api_key_only") return copy("The local client uses an API key. Sign in with a subscription account first.", "本机客户端使用 API Key，请先改用订阅账号登录。", "本機用戶端使用 API Key，請先改用訂閱帳號登入。", "ローカルクライアントは API キーを使用しています。プランのアカウントでサインインしてください。");
    return copy(`Sign in to ${label} on this computer, then check again.`, `请先在本机登录 ${label}，然后重新检查。`, `請先在本機登入 ${label}，然後重新檢查。`, `この端末で ${label} にサインインして再確認してください。`);
  };
  const connect = (id?: string) => run(async active => {
    await connectSubscription(kind, id);
    if (!active()) return;
    await onChanged();
    if (active()) onConnected?.();
  });
  const visible = provider ? accounts.filter(account => account.id === provider.credential_reference) : accounts;
  return <section className="plan-connection-panel" aria-label={`${label} ${copy("local login", "本机登录", "本機登入", "ローカルサインイン")}`} aria-busy={busy}>
    <Alert role="note"><AlertDescription>{copy("This connection shares the local account's plan. Disconnecting removes only this reference and keeps the original login.", "此连接共享本机账号的订阅额度。断开只移除引用，保留原有登录。", "此連線共用本機帳號的訂閱額度。中斷僅移除參照，保留原有登入。", "この接続はローカルアカウントのプラン枠を共有します。解除しても元のサインインは保持されます。")}</AlertDescription></Alert>
    {error && <Alert variant="destructive"><AlertDescription>{copy(`Could not connect ${label}. Check the local login, account access, and network, then retry.`, `无法连接 ${label}，请检查本机登录、账号权限与网络后重试。`, `無法連接 ${label}，請檢查本機登入、帳號權限與網路後重試。`, `${label} に接続できません。サインイン、アカウント権限、ネットワークを確認して再試行してください。`)}</AlertDescription></Alert>}
    {detected && <div className="plan-connection-detected" role="status"><strong>{detected.email || detected.label}</strong><p>{stateText(detected.state)}</p></div>}
    {provider && !busy && visible.length === 0 && <Alert role="status"><AlertDescription>{copy("This reference is disconnected. Reconnect through Subscriptions.", "此引用已断开，请通过“订阅”重新接入。", "此參照已中斷，請透過「訂閱」重新接入。", "この参照は解除されています。サブスクリプションから再接続してください。")}</AlertDescription></Alert>}
    {visible.map(account => <article key={account.id} className="plan-connection-account">
      <div className="plan-account-summary"><div className="plan-account-identity"><strong>{account.label}</strong>{account.email && <span>{account.email}</span>}</div>
        <Badge variant="secondary">{stateText(account.state)}</Badge></div>
      {(catalogs[account.id] || provider) && <ul className="plan-account-model-grid" aria-label={copy("Connected models", "已接入模型", "已接入模型", "接続済みモデル")}>
        {(catalogs[account.id] ?? provider?.models.map(id => ({ id, name: id })) ?? []).map(model => <li key={model.id}><strong>{model.name}</strong>{model.id !== model.name && <code>{model.id}</code>}</li>)}
      </ul>}
      <div className="plan-connection-start">
        {!provider && <Button disabled={busy || account.state !== "ready"} onClick={() => void connect(account.id)}>{copy("Use this account", "使用此账号接入", "使用此帳號接入", "このアカウントを使用")}</Button>}
        <Button variant="secondary" disabled={busy || account.state !== "ready"} onClick={() => void run(async active => {
          const models = await getSubscriptionModels(kind, account.id);
          if (active()) setCatalogs(current => ({ ...current, [account.id]: models }));
          if (provider && active()) { await connectSubscription(kind, account.id); if (active()) await onChanged(); }
        })}>{copy("Refresh models", "刷新模型", "重新整理模型", "モデルを更新")}</Button>
        <Button variant="ghost" disabled={busy} onClick={() => void run(async active => {
          await disconnectSubscription(kind, account.id);
          if (!active()) return;
          const next = await listSubscriptions(kind);
          if (active()) { setAccounts(next); await onChanged(); }
        })}>{copy("Disconnect reference", "断开引用", "中斷參照", "参照を解除")}</Button>
      </div>
    </article>)}
    {!provider && visible.length === 0 && <Button disabled={busy || detected?.state !== "ready"} onClick={() => void connect()}>{copy("Use local login", "使用本机登录接入", "使用本機登入接入", "ローカルサインインを使用")}</Button>}
    <Button variant="ghost" disabled={busy} onClick={() => void run(async active => {
      const [registered, local] = await Promise.all([listSubscriptions(kind), discoverSubscription(kind)]);
      if (active()) { setAccounts(registered); setDetected(local); }
    })}>{copy("Check local login", "检查本机登录", "檢查本機登入", "サインインを確認")}</Button>
  </section>;
}
