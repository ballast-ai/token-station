import { useState } from "react";
import type { ModelDiscoveryView, ProviderView } from "../api";
import {
  ENTERPRISE_PROVIDER_ID,
  ENTERPRISE_PROVIDER_NAME,
} from "../providerPresentation";
import { useLocalizedCopy } from "./LanguageProvider";
import { Input } from "./ui/input";

export { ENTERPRISE_PROVIDER_ID, ENTERPRISE_PROVIDER_NAME } from "../providerPresentation";

export interface EnterpriseConnectionInput {
  name: typeof ENTERPRISE_PROVIDER_ID;
  baseUrl: string;
  apiKey: string;
  model: string;
}

interface EnterpriseConnectionPanelProps {
  existingProvider?: ProviderView | null;
  busy: boolean;
  onVerify: (connection: Pick<EnterpriseConnectionInput, "name" | "baseUrl" | "apiKey">) => Promise<ModelDiscoveryView>;
  onConnect: (connection: EnterpriseConnectionInput) => boolean | Promise<boolean>;
}

export default function EnterpriseConnectionPanel({
  existingProvider = null,
  busy,
  onVerify,
  onConnect,
}: EnterpriseConnectionPanelProps) {
  const { copy } = useLocalizedCopy();
  const [baseUrl, setBaseUrl] = useState("");
  const [apiKey, setApiKey] = useState("");
  const [models, setModels] = useState<string[]>([]);
  const [model, setModel] = useState("");
  const [verifying, setVerifying] = useState(false);
  const [working, setWorking] = useState(false);
  const [message, setMessage] = useState("");

  const invalidateVerification = () => {
    setModels([]);
    setModel("");
    setMessage("");
  };

  const verify = async () => {
    if (!baseUrl.trim() || !apiKey.trim()) {
      setMessage(copy("Enter the Base URL and API key.", "请填写 Base URL 和 API Key。", "請輸入 Base URL 和 API Key。", "Base URL と API キーを入力してください。"));
      return;
    }
    setVerifying(true);
    setMessage("");
    try {
      const discovery = await onVerify({
        name: ENTERPRISE_PROVIDER_ID,
        baseUrl: baseUrl.trim(),
        apiKey: apiKey.trim(),
      });
      if (discovery.source !== "live") {
        setModels([]);
        setModel("");
        setMessage(discovery.warning || copy("Live verification failed.", "实时验证未通过。", "即時驗證未通過。", "ライブ検証に失敗しました。"));
        return;
      }
      setModels(discovery.models);
      setModel("");
      setMessage(discovery.models.length > 0
        ? copy(`${discovery.models.length} models found. Select one to continue.`, `已获取 ${discovery.models.length} 个模型，请选择一个。`, `已取得 ${discovery.models.length} 個模型，請選擇一個。`, `${discovery.models.length} 個のモデルが見つかりました。1つ選択してください。`)
        : copy("No models were returned by this endpoint.", "该地址没有返回可选模型。", "該端點沒有傳回可選模型。", "このエンドポイントからモデルが返されませんでした。"));
    } catch (caught) {
      setModels([]);
      setModel("");
      setMessage(caught instanceof Error ? caught.message : String(caught));
    } finally {
      setVerifying(false);
    }
  };

  const connect = async () => {
    if (!model || models.length === 0) {
      setMessage(copy("Verify the endpoint and select a model.", "请先验证地址并选择模型。", "請先驗證端點並選擇模型。", "エンドポイントを検証してモデルを選択してください。"));
      return;
    }
    setWorking(true);
    setMessage("");
    try {
      const started = await onConnect({
        name: ENTERPRISE_PROVIDER_ID,
        baseUrl: baseUrl.trim(),
        apiKey: apiKey.trim(),
        model,
      });
      if (!started) {
        setMessage(copy("Connection did not complete. Review the error and retry.", "接入未完成，请查看错误后重试。", "接入未完成，請檢視錯誤後重試。", "接続が完了していません。エラーを確認して再試行してください。"));
        return;
      }
      setApiKey("");
      setMessage(copy("Enterprise route added and is being applied.", "企业路由已添加，正在应用配置。", "企業路由已新增，正在套用設定。", "企業ルートを追加し、設定を適用しています。"));
    } finally {
      setWorking(false);
    }
  };

  const disabled = busy || verifying || working;
  if (existingProvider) {
    return (
      <div className="enterprise-connected-summary">
        <span className="enterprise-active-status">{copy("Configured provider", "已配置供应商", "已設定供應商", "設定済みプロバイダー")}</span>
        <strong>{ENTERPRISE_PROVIDER_NAME}</strong>
        <code>{existingProvider.base_url}</code>
        <span>{existingProvider.models.join(" · ")}</span>
        <p className="enterprise-secret-note">{copy(
          "Manage or remove it from the model list before adding it again.",
          "请在模型列表中管理或删除它，再重新添加。",
          "請在模型清單中管理或刪除後再重新新增。",
          "モデル一覧で管理または削除してから再追加してください。",
        )}</p>
      </div>
    );
  }

  return (
    <div className="enterprise-connection-panel">
      <div className="enterprise-provider-identity">
        <span>{copy("Provider", "供应商", "供應商", "プロバイダー")}</span>
        <strong>{ENTERPRISE_PROVIDER_NAME}</strong>
      </div>
      <div className="enterprise-credential-grid">
        <label>
          <span>Base URL</span>
          <Input
            aria-label="Base URL"
            type="url"
            placeholder="https://api.example.com/v1"
            value={baseUrl}
            disabled={disabled}
            onChange={(event) => {
              setBaseUrl(event.target.value);
              invalidateVerification();
            }}
          />
        </label>
        <label>
          <span>{copy("API key", "API Key", "API Key", "APIキー")}</span>
          <Input
            aria-label="API Key"
            type="password"
            autoComplete="off"
            placeholder="sk-…"
            value={apiKey}
            disabled={disabled}
            onChange={(event) => {
              setApiKey(event.target.value);
              invalidateVerification();
            }}
          />
        </label>
        <label>
          <span>{copy("Model", "模型", "模型", "モデル")}</span>
          <select className="select" aria-label={copy("Model", "模型", "模型", "モデル")} value={model} disabled={disabled || models.length === 0} onChange={(event) => setModel(event.target.value)}>
            <option value="">{copy("Select a verified model", "选择已验证模型", "選擇已驗證模型", "検証済みモデルを選択")}</option>
            {models.map((item) => <option value={item} key={item}>{item}</option>)}
          </select>
        </label>
      </div>
      <div className="enterprise-dialog-actions">
        <button className="btn" type="button" disabled={disabled} onClick={() => void verify()}>
          {verifying ? copy("Verifying…", "验证中…", "驗證中…", "検証中…") : copy("Verify and load models", "验证并获取模型", "驗證並取得模型", "検証してモデルを取得")}
        </button>
        <button className="btn primary" type="button" disabled={disabled || !model} onClick={() => void connect()}>
          {working ? copy("Adding…", "添加中…", "新增中…", "追加中…") : copy("Add and use", "添加并使用", "新增並使用", "追加して使用")}
        </button>
      </div>
      {message && <p className="enterprise-connection-status" role="status" aria-live="polite">{message}</p>}
      <p className="enterprise-secret-note">{copy(
        "The API key is stored in the local credential store and is not shown again.",
        "API Key 仅保存到本机凭据存储，接入后不会再次显示。",
        "API Key 僅儲存在本機憑證儲存，接入後不會再次顯示。",
        "APIキーはローカル資格情報ストアに保存され、再表示されません。",
      )}</p>
    </div>
  );
}
