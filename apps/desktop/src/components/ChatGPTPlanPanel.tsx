import { useCallback, useEffect, useRef, useState } from "react";
import {
  cancelChatGPTLogin, connectChatGPTProvider, getChatGPTLoginStatus, getChatGPTModels,
  listChatGPTAccounts, openChatGPTAuthorization, openChatGPTUsage, signOutChatGPT, startChatGPTLogin,
  type ChatGPTAccountView, type ChatGPTLoginAttempt, type ChatGPTPlanModel,
} from "../chatgptPlanApi";
import type { ProviderView } from "../api";
import { Alert, AlertDescription } from "./ui/alert";
import PlanConnectionContent from "./PlanConnectionContent";
import { useLocalizedCopy } from "./LanguageProvider";
import { Button } from "./ui/button";

interface ChatGPTPlanPanelProps {
  provider?: ProviderView;
  onChanged?: () => void | Promise<void>;
  onConnected?: () => void;
}

export default function ChatGPTPlanPanel({ provider, onChanged, onConnected }: ChatGPTPlanPanelProps) {
  const { copy } = useLocalizedCopy();
  const [accounts, setAccounts] = useState<ChatGPTAccountView[]>([]);
  const [catalogs, setCatalogs] = useState<Record<string, ChatGPTPlanModel[]>>({});
  const [busy, setBusy] = useState(true);
  const [attempt, setAttempt] = useState<ChatGPTLoginAttempt | null>(null);
  const [notice, setNotice] = useState("");
  const [error, setError] = useState("");
  const mounted = useRef(false);
  const pending = useRef<string | null>(null);
  const signInButton = useRef<HTMLButtonElement>(null);
  const callbacks = useRef({ onChanged, onConnected, provider });
  callbacks.current = { onChanged, onConnected, provider };
  const copyRef = useRef(copy);
  copyRef.current = copy;

  const refreshAccounts = useCallback(async () => {
    const next = await listChatGPTAccounts();
    if (mounted.current) setAccounts(next);
  }, []);

  useEffect(() => {
    mounted.current = true;
    void refreshAccounts().catch(() => {
      if (mounted.current) setError(copyRef.current("Could not load ChatGPT accounts. Try again.", "无法加载 ChatGPT 账号，请重试。", "無法載入 ChatGPT 帳號，請重試。", "ChatGPT アカウントを読み込めません。再試行してください。"));
    }).finally(() => { if (mounted.current) setBusy(false); });
    return () => {
      mounted.current = false;
      if (pending.current) void cancelChatGPTLogin(pending.current).catch(() => {});
      pending.current = null;
    };
  }, [refreshAccounts]);

  useEffect(() => {
    if (!attempt) return;
    let stopped = false;
    let timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      try {
        const status = await getChatGPTLoginStatus(attempt.id);
        if (stopped || pending.current !== attempt.id) return;
        if (status.state === "pending") {
          timer = setTimeout(() => void poll(), 1200);
          return;
        }
        pending.current = null;
        setAttempt(null);
        if (status.state === "complete") {
          setBusy(true);
          setCatalogs({});
          try {
            await refreshAccounts();
            if (!mounted.current) return;
            const accountId = status.account_id;
            const boundId = callbacks.current.provider?.credential_reference;
            if (!accountId || (callbacks.current.provider && accountId !== boundId)) {
              setError(copyRef.current("The sign-in did not return the expected account. Try again.", "登录没有返回预期账号，请重试。", "登入沒有傳回預期帳號，請重試。", "サインインが必要なアカウントを返しませんでした。再試行してください。"));
              return;
            }
            if (!callbacks.current.provider) await connectChatGPTProvider(accountId);
            if (!mounted.current) return;
            await callbacks.current.onChanged?.();
            if (!mounted.current) return;
            callbacks.current.onConnected?.();
            if (mounted.current) setNotice(copyRef.current("ChatGPT sign-in completed.", "ChatGPT 登录已完成。", "ChatGPT 登入已完成。", "ChatGPT へのサインインが完了しました。"));
          } catch {
            if (mounted.current) setError(copyRef.current("Sign-in completed. Connect the account again to load its models.", "登录已完成，请重新接入账号以加载模型。", "登入已完成，請重新接入帳號以載入模型。", "サインインは完了しました。アカウントを再接続してください。"));
          } finally { if (mounted.current) setBusy(false); }
        } else if (status.state === "failed") {
          setError(copyRef.current("Sign-in failed. Try again.", "登录失败，请重试。", "登入失敗，請重試。", "サインインに失敗しました。再試行してください。"));
        }
      } catch {
        if (!stopped && pending.current === attempt.id) {
          setError(copyRef.current("Could not check sign-in. Cancel and try again.", "无法检查登录状态，请取消后重试。", "無法檢查登入狀態，請取消後重試。", "サインインを確認できません。キャンセルして再試行してください。"));
        }
      }
    };
    void poll();
    return () => { stopped = true; clearTimeout(timer); };
  }, [attempt, refreshAccounts]);

  const beginLogin = async (accountId?: string) => {
    setBusy(true); setError(""); setNotice("");
    let started: ChatGPTLoginAttempt | undefined;
    try {
      started = await startChatGPTLogin(accountId);
      if (!mounted.current) { await cancelChatGPTLogin(started.id); return; }
      pending.current = started.id;
      await openChatGPTAuthorization(started.authorization_url);
      if (mounted.current && pending.current === started.id) setAttempt(started);
    } catch {
      if (started) void cancelChatGPTLogin(started.id).catch(() => {});
      pending.current = null;
      if (mounted.current) setError(copy("Could not start sign-in. Try again.", "无法开始登录，请重试。", "無法開始登入，請重試。", "サインインを開始できません。再試行してください。"));
    } finally { if (mounted.current) setBusy(false); }
  };

  const cancelLogin = async () => {
    if (!attempt) return;
    setBusy(true); setError("");
    try {
      await cancelChatGPTLogin(attempt.id);
      pending.current = null;
      setAttempt(null);
      setNotice(copy("Sign-in cancelled.", "登录已取消。", "登入已取消。", "サインインをキャンセルしました。"));
    } catch {
      setError(copy("Could not cancel sign-in. Try again.", "无法取消登录，请重试。", "無法取消登入，請重試。", "サインインをキャンセルできません。再試行してください。"));
    } finally {
      setBusy(false);
      setTimeout(() => signInButton.current?.focus(), 0);
    }
  };

  const runAction = async (action: () => Promise<void>) => {
    setBusy(true); setError(""); setNotice("");
    try { await action(); }
    catch { if (mounted.current) setError(copy("The operation failed. Check your connection and try again.", "操作失败，请检查连接后重试。", "操作失敗，請檢查連線後重試。", "操作に失敗しました。接続を確認して再試行してください。")); }
    finally { if (mounted.current) setBusy(false); }
  };

  const stateText = (state: ChatGPTAccountView["state"]) => ({
    ready: copy("Ready", "可用", "可用", "準備完了"),
    identity_only: copy("Identity only · plan access not granted", "仅身份登录 · 未授予套餐权限", "僅身分登入 · 未授予方案權限", "本人確認のみ・プラン権限なし"),
    reauthentication_required: copy("Sign-in required", "需要重新登录", "需要重新登入", "再サインインが必要"),
    signed_out: copy("Signed out", "已退出", "已登出", "サインアウト済み"),
  })[state];
  const disabled = busy || attempt !== null;

  const visibleAccounts = provider ? accounts.filter(account => account.id === provider.credential_reference) : accounts;
  const connectAccount = (id: string) => runAction(async () => {
    await connectChatGPTProvider(id);
    if (!mounted.current) return;
    await onChanged?.();
    if (!mounted.current) return;
    onConnected?.();
    if (mounted.current) setNotice(copy("Provider connected. Select this provider on the Agents page.", "供应商已接入，请在 Agent 页面选择该供应商。", "供應商已接入，請在 Agent 頁面選擇此供應商。", "プロバイダーを接続しました。Agent ページで選択してください。"));
  });

  return <section className="plan-connection-panel" aria-label={copy("ChatGPT plan", "ChatGPT 套餐", "ChatGPT 方案", "ChatGPT プラン")} aria-busy={busy}>
    {!provider && <Alert role="note" className="plan-connection-callout"><AlertDescription>{copy(
      "Agents share your ChatGPT plan allowance. Requests stop when it runs out, without switching to a paid API.",
      "Agent 共享你的 ChatGPT 套餐额度。额度用尽时停止请求，不会自动切换到付费 API。",
      "Agent 共用你的 ChatGPT 方案額度。額度用盡時停止請求，不會自動切換到付費 API。",
      "Agent は ChatGPT プランの利用枠を共有します。利用枠がなくなると停止し、有料 API には切り替えません。",
    )}</AlertDescription></Alert>}
    {attempt && <div className="plan-connection-pending">
      <p role="status">{copy("Complete sign-in in your browser.", "请在浏览器中完成登录。", "請在瀏覽器中完成登入。", "ブラウザーでサインインを完了してください。")}</p>
      <Button type="button" variant="secondary" disabled={busy} onClick={() => void cancelLogin()}>{copy("Cancel sign-in", "取消登录", "取消登入", "サインインをキャンセル")}</Button>
    </div>}
    {notice && <p role="status" aria-live="polite" className="plan-connection-notice">{notice}</p>}
    {error && <Alert variant="destructive"><AlertDescription>{error}</AlertDescription></Alert>}
    {provider && !busy && visibleAccounts.length === 0 && <Alert role="status"><AlertDescription>{copy(
      "This connection has no active account. Sign in again or reconnect it from Add model.",
      "此连接没有可用账号，请重新登录或通过“添加模型”重新接入。",
      "此連線沒有可用帳號，請重新登入或透過「新增模型」重新接入。",
      "この接続に有効なアカウントがありません。再サインインするか、モデル追加から再接続してください。",
    )}</AlertDescription></Alert>}
    {visibleAccounts.map(account => <PlanConnectionContent key={account.id} kind="chatgpt" accountId={account.id}
      label={account.label} email={account.email} ready={account.state === "ready"} busy={disabled} managed={Boolean(provider)}
      catalogLoaded={catalogs[account.id] !== undefined}
      models={catalogs[account.id] ?? (provider?.models.map(id => ({ id, name: id })) ?? [])}
      recovery={account.state !== "ready" && <div className="plan-connection-recovery"><Alert role="status"><AlertDescription>{stateText(account.state)}</AlertDescription></Alert>
        <Button ref={signInButton} type="button" disabled={disabled} onClick={() => void beginLogin(account.id)}>{copy("Sign in again", "重新登录", "重新登入", "再サインイン")}</Button></div>}
      connect={!provider && <Button type="button" className="plan-connect-account-button" disabled={disabled || account.state !== "ready"} onClick={() => void connectAccount(account.id)}>{copy("Use this account", "使用此账号接入", "使用此帳號接入", "このアカウントを使用")}</Button>}
      details={account.state === "ready" && <Button ref={provider ? signInButton : undefined} type="button" variant="link" size="sm" disabled={disabled} onClick={() => void beginLogin(account.id)}>{copy("Sign in again", "重新登录", "重新登入", "再サインイン")}</Button>}
      onUsage={() => runAction(openChatGPTUsage)}
      onRefresh={() => runAction(async () => {
        const models = await getChatGPTModels(account.id);
        if (!mounted.current) return;
        await onChanged?.();
        if (mounted.current) setCatalogs(current => ({ ...current, [account.id]: models }));
      })}
      canDisconnect={account.state !== "signed_out"}
      onDisconnect={() => runAction(async () => {
        const result = await signOutChatGPT(account.id);
        if (!mounted.current) return;
        setCatalogs(current => { const next = { ...current }; delete next[account.id]; return next; });
        await refreshAccounts();
        if (!mounted.current) return;
        await onChanged?.();
        if (!mounted.current) return;
        if (!result.remote_revoked) setError(copy("Signed out locally. Remote revocation is unconfirmed. Review connected apps in ChatGPT settings.", "已在本机退出，远端撤销尚未确认。请在 ChatGPT 设置中检查已连接的应用。", "已在本機登出，遠端撤銷尚未確認。請在 ChatGPT 設定中檢查已連接的應用程式。", "ローカルでサインアウトしました。リモートの失効は未確認です。ChatGPT 設定で接続済みアプリを確認してください。"));
        else setNotice(copy("Signed out. Remote access was revoked.", "已退出，远端授权已撤销。", "已登出，遠端授權已撤銷。", "サインアウトし、リモートアクセスを失効しました。"));
      })} />)}
    {(!provider || visibleAccounts.length === 0) && <div className="plan-connection-start">
      <Button ref={signInButton} type="button" disabled={disabled || Boolean(provider && !provider.credential_reference)} onClick={() => void beginLogin(provider?.credential_reference)}>{copy("Continue with ChatGPT", "继续使用 ChatGPT", "繼續使用 ChatGPT", "ChatGPT で続ける")}</Button>
      <Button type="button" variant="ghost" disabled={disabled} onClick={() => void runAction(refreshAccounts)}>{copy("Refresh accounts", "刷新账号", "重新整理帳號", "アカウントを更新")}</Button>
    </div>}
  </section>;
}
