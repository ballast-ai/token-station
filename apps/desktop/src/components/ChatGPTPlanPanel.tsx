import { useCallback, useEffect, useRef, useState } from "react";
import {
  cancelChatGPTLogin, connectChatGPTProvider, getChatGPTLoginStatus, getChatGPTModels,
  listChatGPTAccounts, openChatGPTAuthorization, openChatGPTUsage, signOutChatGPT, startChatGPTLogin,
  type ChatGPTAccountView, type ChatGPTLoginAttempt, type ChatGPTPlanModel,
} from "../chatgptPlanApi";
import { useLocalizedCopy } from "./LanguageProvider";
import { Button } from "./ui/button";
import "./ChatGPTPlanPanel.css";

interface ChatGPTPlanPanelProps { onChanged?: () => void | Promise<void> }

export default function ChatGPTPlanPanel({ onChanged }: ChatGPTPlanPanelProps) {
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
            if (mounted.current) setNotice(copyRef.current("ChatGPT sign-in completed.", "ChatGPT 登录已完成。", "ChatGPT 登入已完成。", "ChatGPT へのサインインが完了しました。"));
          } catch {
            if (mounted.current) setError(copyRef.current("Sign-in completed. Refresh accounts to load the connection.", "登录已完成，请刷新账号以加载连接。", "登入已完成，請重新整理帳號以載入連線。", "サインインは完了しました。アカウントを更新してください。"));
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

  return <section className="chatgpt-plan-panel" aria-labelledby="chatgpt-plan-title">
    <div className="chatgpt-plan-heading">
      <div><h2 id="chatgpt-plan-title">ChatGPT {copy("plan", "套餐", "方案", "プラン")}</h2>
        <p>{copy("Experimental connection · shared plan allowance", "实验性接入 · 共享套餐额度", "實驗性接入 · 共用方案額度", "試験的な接続・プラン枠を共有")}</p></div>
      <Button ref={signInButton} type="button" disabled={disabled} onClick={() => void beginLogin()}>Continue with ChatGPT</Button>
    </div>
    <p className="chatgpt-plan-note">{copy(
      "Agents share your ChatGPT plan allowance. This is not API credit, and no dollar cost is estimated. Temperature and output limits are unsupported. Requests stop when the allowance is exhausted, without switching to a paid API.",
      "Agent 共享你的 ChatGPT 套餐额度，不是 API 余额，也不估算美元费用。暂不支持温度和输出上限。额度耗尽时停止请求，不会切换到付费 API。",
      "Agent 共用你的 ChatGPT 方案額度，不是 API 餘額，也不估算美元費用。暫不支援溫度和輸出上限。額度用盡時停止請求，不會切換到付費 API。",
      "Agent は ChatGPT プランの利用枠を共有します。API クレジットではなく、ドル費用も推定しません。温度と出力上限は未対応です。利用枠を使い切ると停止し、有料 API へ切り替えません。",
    )}</p>
    <div className="chatgpt-plan-actions">
      <Button type="button" variant="link" onClick={() => void runAction(openChatGPTUsage)} disabled={busy}>{copy("View ChatGPT usage", "查看 ChatGPT 用量", "檢視 ChatGPT 用量", "ChatGPT の使用量を表示")}</Button>
      <Button type="button" variant="ghost" onClick={() => void runAction(refreshAccounts)} disabled={disabled}>{copy("Refresh accounts", "刷新账号", "重新整理帳號", "アカウントを更新")}</Button>
    </div>
    {attempt && <div className="chatgpt-plan-pending">
      <p role="status">{copy("Complete sign-in in your browser.", "请在浏览器中完成登录。", "請在瀏覽器中完成登入。", "ブラウザーでサインインを完了してください。")}</p>
      <Button type="button" variant="secondary" disabled={busy} onClick={() => void cancelLogin()}>{copy("Cancel sign-in", "取消登录", "取消登入", "サインインをキャンセル")}</Button>
    </div>}
    {notice && <p role="status" aria-live="polite">{notice}</p>}
    {error && <p role="alert" className="chatgpt-plan-error">{error}</p>}
    <ul className="chatgpt-plan-accounts">{accounts.map(account => <li key={account.id} className="chatgpt-plan-account">
      <div className="chatgpt-plan-account-summary"><strong>{account.label || account.email || account.id}</strong>
        {account.email && account.email !== account.label && <span>{account.email}</span>}
        <span>{copy("Connection", "连接", "連線", "接続")}: <code>{account.id}</code></span>
        <span>{stateText(account.state)}</span></div>
      <div className="chatgpt-plan-actions">
        <Button type="button" disabled={disabled || account.state !== "ready"} onClick={() => void runAction(async () => {
          await connectChatGPTProvider(account.id);
          await onChanged?.();
          setNotice(copy("Provider connected. Select this provider on the Agents page.", "供应商已接入，请在 Agent 页面选择该供应商。", "供應商已接入，請在 Agent 頁面選擇此供應商。", "プロバイダーを接続しました。Agent ページで選択してください。"));
        })}>{copy("Connect as provider", "接入为供应商", "接入為供應商", "プロバイダーとして接続")}</Button>
        <Button type="button" variant="secondary" disabled={disabled || account.state !== "ready"} onClick={() => void runAction(async () => {
          const models = await getChatGPTModels(account.id);
          setCatalogs(current => ({ ...current, [account.id]: models }));
        })}>{copy("Refresh models", "刷新模型", "重新整理模型", "モデルを更新")}</Button>
        <Button type="button" variant="ghost" disabled={disabled} onClick={() => void beginLogin(account.id)}>{copy("Sign in again", "重新登录", "重新登入", "再サインイン")}</Button>
        <Button type="button" variant="ghost" disabled={disabled || account.state === "signed_out"} onClick={() => void runAction(async () => {
          const result = await signOutChatGPT(account.id);
          setCatalogs(current => { const next = { ...current }; delete next[account.id]; return next; });
          await refreshAccounts();
          if (!result.remote_revoked) setError(copy("Signed out locally. Remote revocation is unconfirmed. Review connected apps in ChatGPT settings.", "已在本机退出，远端撤销尚未确认。请在 ChatGPT 设置中检查已连接的应用。", "已在本機登出，遠端撤銷尚未確認。請在 ChatGPT 設定中檢查已連接的應用程式。", "ローカルでサインアウトしました。リモートの失効は未確認です。ChatGPT 設定で接続済みアプリを確認してください。"));
          else setNotice(copy("Signed out. Remote access was revoked.", "已退出，远端授权已撤销。", "已登出，遠端授權已撤銷。", "サインアウトし、リモートアクセスを失効しました。"));
        })}>{copy("Sign out", "退出登录", "登出", "サインアウト")}</Button>
      </div>
      {catalogs[account.id] && <div className="chatgpt-plan-models">
        {catalogs[account.id].length === 0 ? <p>{copy("No available models returned.", "没有返回可用模型。", "沒有傳回可用模型。", "利用可能なモデルが返されませんでした。")}</p>
          : <ul aria-label={copy("Available models", "可用模型", "可用模型", "利用可能なモデル")}>{catalogs[account.id].map(model => <li key={model.id}><strong>{model.name}</strong> <code>{model.id}</code></li>)}</ul>}
      </div>}
    </li>)}</ul>
  </section>;
}
