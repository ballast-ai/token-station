import { useState } from "react";
import type { ModelDiscoveryView, ProviderView } from "../api";
import {
  ENTERPRISE_PROVIDER_ID,
  ENTERPRISE_PROVIDER_NAME,
} from "../providerPresentation";
import { useLocalizedCopy } from "./LanguageProvider";
import { Button } from "./ui/button";
import { Field, FieldGroup, FieldLabel } from "./ui/field";
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
  const extendingProvider = existingProvider?.managed_route === true;
  const configuredModels = new Set(extendingProvider ? existingProvider.models : []);
  const [baseUrl, setBaseUrl] = useState(extendingProvider ? existingProvider.base_url : "");
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
      setMessage(extendingProvider
        ? copy("Model added and selected.", "模型已添加并切换使用。", "模型已新增並切換使用。", "モデルを追加して選択しました。")
        : copy("Enterprise route added and is being applied.", "企业路由已添加，正在应用配置。", "企業路由已新增，正在套用設定。", "企業ルートを追加し、設定を適用しています。"));
    } finally {
      setWorking(false);
    }
  };

  const disabled = busy || verifying || working;
  if (existingProvider && !extendingProvider) {
    return (
      <div className="enterprise-provider-collision" role="status">
        <span>{copy("Reserved provider name is already in use", "保留的供应商名称已被占用", "保留的供應商名稱已被使用", "予約済みプロバイダー名は既に使用されています")}</span>
        <strong>{ENTERPRISE_PROVIDER_NAME}</strong>
        <code>{existingProvider.base_url}</code>
        <p className="enterprise-secret-note">{copy(
          "Rename or remove this ordinary provider before using the managed enterprise flow.",
          "请先重命名或删除这个普通供应商，再使用企业托管流程。",
          "請先重新命名或刪除此一般供應商，再使用企業託管流程。",
          "この通常プロバイダーの名前を変更するか削除してから、企業管理フローを使用してください。",
        )}</p>
      </div>
    );
  }

  return (
    <div className="enterprise-connection-panel">
      <div className="enterprise-provider-identity">
        <div>
          <span>{copy("Provider", "供应商", "供應商", "プロバイダー")}</span>
          <strong>{ENTERPRISE_PROVIDER_NAME}</strong>
        </div>
        {extendingProvider && (
          <span className="enterprise-existing-count">{copy(
            `${existingProvider.models.length} models configured`,
            `已配置 ${existingProvider.models.length} 个模型`,
            `已設定 ${existingProvider.models.length} 個模型`,
            `${existingProvider.models.length} 個のモデルを設定済み`,
          )}</span>
        )}
      </div>
      <FieldGroup className="enterprise-credential-grid">
        <Field>
          <FieldLabel htmlFor="enterprise-base-url">Base URL</FieldLabel>
          <Input
            id="enterprise-base-url"
            aria-label="Base URL"
            type="url"
            placeholder="https://api.example.com/v1"
            value={baseUrl}
            disabled={disabled || extendingProvider}
            onChange={(event) => {
              setBaseUrl(event.target.value);
              invalidateVerification();
            }}
          />
        </Field>
        <Field>
          <FieldLabel htmlFor="enterprise-api-key">{copy("API key", "API Key", "API Key", "APIキー")}</FieldLabel>
          <Input
            id="enterprise-api-key"
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
        </Field>
      </FieldGroup>
      <div className="enterprise-verify-actions">
        <Button variant="secondary" type="button" disabled={disabled} onClick={() => void verify()}>
          {verifying ? copy("Verifying…", "验证中…", "驗證中…", "検証中…") : copy("Verify and load models", "验证并获取模型", "驗證並取得模型", "検証してモデルを取得")}
        </Button>
      </div>
      {message && <p className="enterprise-connection-status" role="status" aria-live="polite">{message}</p>}
      {models.length > 0 && (
        <section className="enterprise-model-picker" aria-labelledby="enterprise-model-picker-label">
          <div className="enterprise-model-picker-heading">
            <strong id="enterprise-model-picker-label">{copy("Model", "模型", "模型", "モデル")}</strong>
            <span>{copy("Select one", "选择一个", "選擇一個", "1つ選択")}</span>
          </div>
          <div className="enterprise-model-options" role="radiogroup" aria-labelledby="enterprise-model-picker-label">
            {models.map((item) => {
              const configured = configuredModels.has(item);
              const configuredLabel = copy("Configured", "已添加", "已新增", "追加済み");
              return (
                <label
                  className="enterprise-model-option"
                  data-configured={configured || undefined}
                  key={item}
                >
                  <input
                    type="radio"
                    name="enterprise-model"
                    value={item}
                    aria-label={configured ? `${item}, ${configuredLabel}` : item}
                    checked={model === item}
                    disabled={disabled || configured}
                    onChange={() => setModel(item)}
                  />
                  <span className="enterprise-model-name">{item}</span>
                  <span className="enterprise-model-option-status" aria-hidden="true">
                    {configured
                      ? configuredLabel
                      : model === item
                        ? copy("Selected", "已选择", "已選擇", "選択済み")
                        : ""}
                  </span>
                </label>
              );
            })}
          </div>
        </section>
      )}
      <div className="enterprise-dialog-actions">
        <Button type="button" disabled={disabled || !model} onClick={() => void connect()}>
          {working ? copy("Adding…", "添加中…", "新增中…", "追加中…") : copy("Add and use", "添加并使用", "新增並使用", "追加して使用")}
        </Button>
      </div>
      <p className="enterprise-secret-note">{copy(
        "The API key is stored in the local credential store and is not shown again.",
        "API Key 仅保存到本机凭据存储，接入后不会再次显示。",
        "API Key 僅儲存在本機憑證儲存，接入後不會再次顯示。",
        "APIキーはローカル資格情報ストアに保存され、再表示されません。",
      )}</p>
    </div>
  );
}
