import { useEffect, useRef, useState } from "react";
import { connectCodexProvider, discoverCodexAccount, disconnectCodexAccount, getCodexModels, listCodexAccounts, type LocalCodexAccount } from "../codexAccountsApi";
import { openChatGPTUsage, type ChatGPTPlanModel } from "../chatgptPlanApi";
import type { ProviderView } from "../api";
import { Alert, AlertDescription } from "./ui/alert";
import PlanConnectionContent from "./PlanConnectionContent";
import { useLocalizedCopy } from "./LanguageProvider";
import { Button } from "./ui/button";

export default function LocalCodexPanel({ provider, onChanged, onConnected }: {
  provider?: ProviderView;
  onChanged?: () => void | Promise<void>;
  onConnected?: () => void;
}) {
  const { copy } = useLocalizedCopy();
  const [accounts, setAccounts] = useState<LocalCodexAccount[]>([]);
  const [detected, setDetected] = useState<LocalCodexAccount | null>(null);
  const [catalogs, setCatalogs] = useState<Record<string, ChatGPTPlanModel[]>>({});
  const [busy, setBusy] = useState(true);
  const [notice, setNotice] = useState("");
  const [error, setError] = useState("");
  const mounted = useRef(false);
  const copyRef = useRef(copy); copyRef.current = copy;
  const refresh = async () => {
    const next = await listCodexAccounts();
    if (mounted.current) setAccounts(next);
  };
  useEffect(() => {
    mounted.current = true;
    void listCodexAccounts().then(next => { if (mounted.current) setAccounts(next); })
      .catch(() => { if (mounted.current) setError(copyRef.current("Could not load local Codex references. Check the local login and retry.", "无法加载本机 Codex 引用，请检查登录后重试。", "無法載入本機 Codex 參照，請檢查登入後重試。", "ローカル Codex の参照を読み込めません。サインインを確認してください。")); })
      .finally(() => { if (mounted.current) setBusy(false); });
    return () => { mounted.current = false; };
  }, []);
  const run = async (message: string, action: () => Promise<void>) => {
    setBusy(true); setError(""); setNotice("");
    try { await action(); }
    catch { if (mounted.current) setError(message); }
    finally { if (mounted.current) setBusy(false); }
  };
  const stateText = (state: LocalCodexAccount["state"]) => {
    switch (state) {
      case "ready": return copy("Local login ready", "本机登录可用", "本機登入可用", "ローカルのサインインを利用できます");
      case "missing": return copy("Sign in to Codex on this computer, then check again.", "请先在本机登录 Codex，然后重新检查。", "請先在本機登入 Codex，然後重新檢查。", "この端末で Codex にサインインして再確認してください。");
      case "api_key_only": return copy("This Codex login uses an API key. Sign in with a ChatGPT plan account in Codex.", "当前 Codex 使用 API Key，请在 Codex 中改用 ChatGPT 套餐账号登录。", "目前 Codex 使用 API Key，請在 Codex 中改用 ChatGPT 方案帳號登入。", "Codex は API キーを使用しています。ChatGPT プランでサインインしてください。");
      case "expired": return copy("Session expired. Try connecting again. Renew the login in Codex if it cannot refresh.", "登录已过期，请先尝试重新接入。若无法自动刷新，再到 Codex 中更新登录。", "登入已過期，請先嘗試重新接入。若無法自動重新整理，再到 Codex 中更新登入。", "セッションの期限が切れました。再接続を試し、更新できない場合は Codex でサインインを更新してください。");
      case "unsupported_storage": return copy("This Codex credential storage is not supported. Use a supported local Codex login, then check again.", "暂不支持当前 Codex 凭据存储方式，请使用受支持的本机登录后重新检查。", "暫不支援目前 Codex 憑證儲存方式，請使用受支援的本機登入後重新檢查。", "この Codex 認証情報の保存方式は未対応です。対応するローカルのサインインを使用してください。");
      case "account_changed": return copy("The local Codex account has changed. Disconnect this reference, then connect the current login explicitly.", "本机 Codex 账号已变更，请断开旧引用，再明确接入当前登录。", "本機 Codex 帳號已變更，請中斷舊參照，再明確接入目前登入。", "Codex アカウントが変更されました。古い参照を解除して現在のアカウントを接続してください。");
      default: return copy("The local Codex login data could not be read. Repair the login in Codex and check again.", "无法读取本机 Codex 登录数据，请在 Codex 中修复登录后重新检查。", "無法讀取本機 Codex 登入資料，請在 Codex 中修復登入後重新檢查。", "Codex のサインイン情報を読み込めません。Codex で修復して再確認してください。");
    }
  };
  const checkFailure = copy("Could not check the local Codex login. Retry after signing in to Codex.", "无法检查本机 Codex 登录，请登录 Codex 后重试。", "無法檢查本機 Codex 登入，請登入 Codex 後重試。", "Codex のサインインを確認できません。サインインして再試行してください。");
  const connect = (id?: string) => run(copy("Could not connect the local Codex account. Check the local login and its model access, then retry.", "无法接入本机 Codex 账号，请检查登录和模型权限后重试。", "無法接入本機 Codex 帳號，請檢查登入和模型權限後重試。", "Codex を接続できません。サインインとモデルへのアクセスを確認してください。"), async () => {
    try { await connectCodexProvider(id); }
    catch (failure) {
      try { const next = await discoverCodexAccount(); if (mounted.current) setDetected(next); } catch { /* Keep only the safe operation error. */ }
      throw failure;
    }
    await onChanged?.();
    if (!mounted.current) return;
    await refresh();
    if (!mounted.current) return;
    onConnected?.();
    if (mounted.current) setDetected(null);
    if (mounted.current) setNotice(copy("Local Codex provider connected. Select the provider for your Agents.", "本机 Codex 供应商已接入，请为 Agent 选择该供应商。", "本機 Codex 供應商已接入，請為 Agent 選擇此供應商。", "Codex を接続しました。Agent 用のプロバイダーを選択してください。"));
  });
  const visibleAccounts = provider ? accounts.filter(account => account.id === provider.credential_reference) : accounts;
  return <section className="plan-connection-panel" aria-label={copy("Local Codex login", "本机 Codex 登录", "本機 Codex 登入", "ローカル Codex サインイン")} aria-busy={busy}>
    {!provider && <Alert role="note" className="plan-connection-callout"><AlertDescription>{copy("Agents share this account's subscription allowance. No API cost is estimated and exhausted allowance does not switch to a paid API. Disconnecting here does not sign Codex out.", "Agent 共享该账号的套餐额度，不估算 API 费用。额度用尽时停止请求，不会切换到付费 API。在此断开不会退出 Codex 登录。", "Agent 共用此帳號的方案額度，不估算 API 費用。額度用盡時停止請求，不會切換至付費 API。在此中斷不會登出 Codex。", "Agent はこのアカウントのプラン枠を共有します。API 費用は推定せず、利用枠を使い切ると停止します。接続を解除しても Codex はサインインしたままです。")}</AlertDescription></Alert>}
    {detected && <div className="plan-connection-detected" role="status"><strong>{detected.label || detected.email}</strong><p>{stateText(detected.state)}</p></div>}
    {notice && <p role="status" aria-live="polite" className="plan-connection-notice">{notice}</p>}
    {error && <Alert variant="destructive"><AlertDescription>{error}</AlertDescription></Alert>}
    {provider && !busy && visibleAccounts.length === 0 && <Alert role="status"><AlertDescription>{copy(
      "This local reference is disconnected. Use Add model to reconnect the intended Codex login.",
      "此本机引用已断开，请通过“添加模型”重新接入目标 Codex 登录。",
      "此本機參照已中斷，請透過「新增模型」重新接入目標 Codex 登入。",
      "この参照は解除されています。モデル追加から必要な Codex サインインを再接続してください。",
    )}</AlertDescription></Alert>}
    {visibleAccounts.map(account => <PlanConnectionContent key={account.id} kind="codex" accountId={account.id}
      label={account.label} email={account.email} ready={account.state === "ready"} busy={busy} managed={Boolean(provider)}
      catalogLoaded={catalogs[account.id] !== undefined}
      models={catalogs[account.id] ?? (provider?.models.map(id => ({ id, name: id })) ?? [])}
      recovery={account.state !== "ready" && <div className="plan-connection-recovery"><Alert role="status"><AlertDescription>{stateText(account.state)}</AlertDescription></Alert>
        {account.state === "expired" && <Button type="button" disabled={busy} onClick={() => void connect(account.id)}>{copy("Reconnect", "重新接入", "重新接入", "再接続")}</Button>}</div>}
      connect={!provider && <Button type="button" className="plan-connect-account-button" disabled={busy || account.state !== "ready"} onClick={() => void connect(account.id)}>{copy("Use this account", "使用此账号接入", "使用此帳號接入", "このアカウントを使用")}</Button>}
      onUsage={() => run(copy("Could not open ChatGPT usage. Try again.", "无法打开 ChatGPT 用量，请重试。", "無法開啟 ChatGPT 用量，請重試。", "ChatGPT の使用量を表示できません。再試行してください。"), openChatGPTUsage)}
      onRefresh={() => run(copy("Could not refresh Codex models. Check the local login and retry.", "无法刷新 Codex 模型，请检查本机登录后重试。", "無法重新整理 Codex 模型，請檢查本機登入後重試。", "Codex のモデルを更新できません。サインインを確認してください。"), async () => {
        const models = await getCodexModels(account.id);
        if (!mounted.current) return;
        await onChanged?.();
        if (mounted.current) setCatalogs(current => ({ ...current, [account.id]: models }));
      })}
      onDisconnect={() => run(copy("Could not disconnect the Codex reference. Try again.", "无法断开 Codex 引用，请重试。", "無法中斷 Codex 參照，請重試。", "Codex の参照を解除できません。再試行してください。"), async () => {
        await disconnectCodexAccount(account.id);
        await onChanged?.();
        if (!mounted.current) return;
        await refresh();
        if (!mounted.current) return;
        if (mounted.current) {
          setCatalogs(current => { const next = { ...current }; delete next[account.id]; return next; });
          setNotice(copy("Reference disconnected. Codex remains signed in. Requests through the old provider stop until reconnected.", "引用已断开，Codex 仍保持登录。旧供应商的请求会停止，重新接入后才会恢复。", "參照已中斷，Codex 仍保持登入。舊供應商的請求會停止，重新接入後才會恢復。", "参照を解除しました。Codex はサインインしたままです。再接続まで旧プロバイダーのリクエストは停止します。"));
        }
      })} />)}
    <div className="plan-connection-start">
      {!provider && visibleAccounts.length === 0 && <Button type="button" disabled={busy} onClick={() => void connect()}>{copy("Use local Codex login", "使用本机 Codex 登录", "使用本機 Codex 登入", "ローカル Codex に接続")}</Button>}
      <Button type="button" variant="ghost" disabled={busy} onClick={() => void run(checkFailure, async () => {
        const next = await discoverCodexAccount(); if (mounted.current) setDetected(next); await refresh();
      })}>{copy("Check local login", "检查本机登录", "檢查本機登入", "サインインを確認")}</Button>
    </div>
  </section>;
}
