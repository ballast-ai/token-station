import { useMemo, useState } from "react";
import { type ProviderView } from "../api";
import { useLocalizedCopy } from "./LanguageProvider";
import { Input } from "./ui/input";

export interface EnterpriseConnectionInput {
  name: string;
  baseUrl: string;
  apiKey: string;
}

interface EnterpriseConnectionPanelProps {
  providers: ProviderView[];
  busy: boolean;
  onConnect: (connection: EnterpriseConnectionInput) => boolean | Promise<boolean>;
}

function endpointProviderName(baseUrl: string, providers: ProviderView[]): string {
  let host = "endpoint";
  try {
    host = new URL(baseUrl).hostname || host;
  } catch {
    // The backend returns the actionable URL validation error during verification.
  }
  const stem = `enterprise_${host.toLocaleLowerCase().replace(/[^a-z0-9]+/g, "_").replace(/^_|_$/g, "") || "endpoint"}`;
  const names = new Set(providers.map((provider) => provider.name));
  if (!names.has(stem)) return stem;
  let suffix = 2;
  while (names.has(`${stem}_${suffix}`)) suffix += 1;
  return `${stem}_${suffix}`;
}

export default function EnterpriseConnectionPanel({
  providers,
  busy,
  onConnect,
}: EnterpriseConnectionPanelProps) {
  const { copy } = useLocalizedCopy();
  const [baseUrl, setBaseUrl] = useState("");
  const [apiKey, setApiKey] = useState("");
  const [accountName, setAccountName] = useState("");
  const [working, setWorking] = useState(false);
  const [message, setMessage] = useState("");
  const suggestedName = useMemo(
    () => endpointProviderName(baseUrl.trim(), providers),
    [baseUrl, providers],
  );
  const resolvedName = accountName.trim() || suggestedName;
  const explicitNameExists = Boolean(
    accountName.trim() && providers.some((provider) => provider.name === accountName.trim()),
  );

  const clearMessage = () => setMessage("");

  const connect = async () => {
    if (!baseUrl.trim() || !apiKey.trim()) {
      setMessage(copy("Enter the Base URL and API key.", "请填写 Base URL 和 API Key。", "請輸入 Base URL 和 API Key。", "Base URL と API キーを入力してください。"));
      return;
    }
    if (explicitNameExists) {
      setMessage(copy("This account name already exists.", "该账户名称已存在。", "該帳號名稱已存在。", "このアカウント名は既に存在しています。"));
      return;
    }
    setWorking(true);
    setMessage("");
    try {
      const connection = {
        name: resolvedName,
        baseUrl: baseUrl.trim(),
        apiKey: apiKey.trim(),
      };
      const started = await onConnect(connection);
      if (!started) {
        setMessage(copy(
          "Connection did not complete. Review the error and retry.",
          "接入未完成，请查看错误后重试。", "接入未完成，請檢視錯誤後重試。", "接続が完了していません。エラーを確認し、再試行してください。"
        ));
        return;
      }
      setApiKey("");
      setBaseUrl("");
      setAccountName("");
      setMessage(copy(
        "Enterprise route connected. Applying configuration…",
        "企业路由已接入，正在应用配置…", "企業路由已接入，正在應用配置…", "企業ルーティングが接続されました。設定を適用中…"
      ));
    } finally {
      setWorking(false);
    }
  };

  const disabled = busy || working;
  return (
    <section className="panel enterprise-connection-panel" aria-label={copy("Enterprise route connection", "企业路由接入", "企業路由接入", "企業ルーティング接続")}>
      <div className="panel-head split-heading">
        <div>
          <h2>{copy("Connect enterprise route", "接入企业路由", "接入企業路由", "企業ルーティング接続")}</h2>
          <p className="sub">{copy(
            "Enter the managed endpoint and credential. Models and routing policy stay on the enterprise service.",
            "填写企业路由地址与凭据；模型和路由策略均由企业服务管理。", "請填寫企業路由地址與憑據；模型和路由策略均由企業服務管理。", "企業ルーティングのエンドポイントと資格情報を入力してください。モデルとルーティングポリシーは企業サービスで管理されます。"
          )}</p>
        </div>
        <button className="btn primary" type="button" disabled={disabled} onClick={() => void connect()}>
          {working ? copy("Connecting…", "接入中…", "連線中…", "接続中…") : copy("Connect and use", "接入并使用", "連線並使用", "接続して使用")}
        </button>
      </div>

      <div className="enterprise-credential-grid">
        <label>
          <span>{copy("Base URL", "Base URL", "Base URL", "Base URL")}</span>
          <Input
            aria-label="Base URL"
            type="url"
            placeholder="https://api.example.com/v1"
            value={baseUrl}
            disabled={disabled}
            onChange={(event) => {
              setBaseUrl(event.target.value);
              clearMessage();
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
              clearMessage();
            }}
          />
        </label>
        <label>
          <span>{copy("Account name (optional)", "账户名称（可选）", "帳號名稱（可選）", "アカウント名（オプション）")}</span>
          <Input
            aria-label={copy("Account name", "账户名称", "帳號名稱", "アカウント名")}
            placeholder={suggestedName}
            value={accountName}
            disabled={disabled}
            onChange={(event) => {
              setAccountName(event.target.value);
              clearMessage();
            }}
          />
        </label>
      </div>

      {message && <p className="enterprise-connection-status" role="status" aria-live="polite">{message}</p>}
      <p className="enterprise-secret-note">{copy(
        "The API key is stored in the local credential store and is not shown again.",
        "API Key 仅保存到本机凭据存储，接入后不会再次显示。", "API Key 會儲存至本機憑證儲存處，接入後不會再次顯示。", "APIキーはローカルの資格情報ストアに保存され、接続後は再度表示されません。"
      )}</p>
    </section>
  );
}
