import { useMemo, useState } from "react";
import {
  addProvider,
  discoverProviderModels,
  type ProviderView,
  type StateView,
} from "../api";
import { humanizeAppError } from "../errors";
import { useLocalizedCopy } from "./LanguageProvider";
import { Input } from "./ui/input";

interface EnterpriseConnectionPanelProps {
  providers: ProviderView[];
  busy: boolean;
  onConnected: (state: StateView, providerName: string, models: string[]) => void;
}

interface VerifiedConnection {
  name: string;
  baseUrl: string;
  apiKey: string;
}

function endpointProviderName(baseUrl: string, providers: ProviderView[]): string {
  let host = "endpoint";
  try {
    host = new URL(baseUrl).hostname || host;
  } catch {
    // The backend returns the actionable URL validation error during verification.
  }
  const stem = `enterprise-${host.toLocaleLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "") || "endpoint"}`;
  const names = new Set(providers.map((provider) => provider.name));
  if (!names.has(stem)) return stem;
  let suffix = 2;
  while (names.has(`${stem}-${suffix}`)) suffix += 1;
  return `${stem}-${suffix}`;
}

export default function EnterpriseConnectionPanel({
  providers,
  busy,
  onConnected,
}: EnterpriseConnectionPanelProps) {
  const { copy, language } = useLocalizedCopy();
  const [baseUrl, setBaseUrl] = useState("");
  const [apiKey, setApiKey] = useState("");
  const [accountName, setAccountName] = useState("");
  const [models, setModels] = useState<string[]>([]);
  const [selectedModels, setSelectedModels] = useState<string[]>([]);
  const [working, setWorking] = useState(false);
  const [message, setMessage] = useState("");
  const [completedName, setCompletedName] = useState("");
  const [verifiedConnection, setVerifiedConnection] = useState<VerifiedConnection | null>(null);
  const suggestedName = useMemo(
    () => endpointProviderName(baseUrl.trim(), providers),
    [baseUrl, providers],
  );
  const resolvedName = accountName.trim() || suggestedName;
  const explicitNameExists = Boolean(
    accountName.trim() && providers.some((provider) => provider.name === accountName.trim()),
  );

  const invalidateVerification = () => {
    setModels([]);
    setSelectedModels([]);
    setVerifiedConnection(null);
    setCompletedName("");
    setMessage("");
  };

  const verify = async () => {
    if (!baseUrl.trim() || !apiKey.trim()) {
      setMessage(copy("Enter the Base URL and API key.", "请填写 Base URL 和 API Key。"));
      return;
    }
    if (explicitNameExists) {
      setMessage(copy("This account name already exists.", "该账户名称已存在。"));
      return;
    }
    setWorking(true);
    setMessage("");
    setCompletedName("");
    setModels([]);
    setSelectedModels([]);
    setVerifiedConnection(null);
    try {
      const connection = {
        name: resolvedName,
        baseUrl: baseUrl.trim(),
        apiKey: apiKey.trim(),
      };
      const discovery = await discoverProviderModels(
        connection.name,
        connection.baseUrl,
        connection.apiKey,
      );
      if (discovery.source !== "live") {
        setMessage(discovery.warning
          ? humanizeAppError(discovery.warning, language)
          : copy(
              "Live credential verification failed. Retry the connection.",
              "实时凭据验证失败，请重试连接。",
            ));
        return;
      }
      setModels(discovery.models);
      setVerifiedConnection(discovery.models.length > 0 ? connection : null);
      setMessage(discovery.models.length > 0
        ? copy(
            `Connection verified. Choose the models to connect.`,
            "连接验证成功，请选择要接入的模型。",
          )
        : copy("Connection succeeded, but no models were returned.", "连接成功，但接口未返回可用模型。"));
    } catch (caught) {
      setMessage(humanizeAppError(caught, language));
    } finally {
      setWorking(false);
    }
  };

  const connect = async () => {
    const currentConnection = {
      name: resolvedName,
      baseUrl: baseUrl.trim(),
      apiKey: apiKey.trim(),
    };
    if (
      !verifiedConnection
      || verifiedConnection.name !== currentConnection.name
      || verifiedConnection.baseUrl !== currentConnection.baseUrl
      || verifiedConnection.apiKey !== currentConnection.apiKey
    ) {
      invalidateVerification();
      setMessage(copy(
        "Connection details changed. Verify the endpoint again.",
        "连接信息已变更，请重新验证端点。",
      ));
      return;
    }
    if (selectedModels.length === 0) {
      setMessage(copy("Select at least one model.", "请至少选择一个模型。"));
      return;
    }
    setWorking(true);
    setMessage("");
    try {
      const next = await addProvider(
        resolvedName,
        baseUrl.trim(),
        selectedModels,
        apiKey.trim(),
        false,
        "store",
        null,
        "openai-compatible",
      );
      const connectedModels = [...selectedModels];
      onConnected(next, resolvedName, connectedModels);
      setCompletedName(resolvedName);
      setApiKey("");
      setBaseUrl("");
      setAccountName("");
      setModels([]);
      setSelectedModels([]);
      setVerifiedConnection(null);
      setMessage(copy(
        `${resolvedName} connected. Its models are available in global and Agent routing.`,
        `${resolvedName} 已接入，可在全局路由或 Agent 路由中使用其模型。`,
      ));
    } catch (caught) {
      setMessage(humanizeAppError(caught, language));
    } finally {
      setWorking(false);
    }
  };

  const disabled = busy || working;
  return (
    <section className="panel enterprise-connection-panel" aria-label={copy("Enterprise endpoint connection", "企业端点接入")}>
      <div className="panel-head split-heading">
        <div>
          <h2>{copy("Connect enterprise account", "接入企业账户")}</h2>
          <p className="sub">{copy(
            "Verify an OpenAI-compatible endpoint, then choose which returned models to connect.",
            "填写企业接口地址与密钥，验证成功后选择要接入的模型。",
          )}</p>
        </div>
        <button className="btn" type="button" disabled={disabled} onClick={() => void verify()}>
          {working && models.length === 0 ? copy("Verifying…", "验证中…") : copy("Verify connection", "验证连接")}
        </button>
      </div>

      <ol className="enterprise-connection-steps" aria-label={copy("Connection flow", "接入流程")}>
        <li data-state={models.length > 0 || completedName ? "complete" : "active"}>
          <span>1</span>
          <div><strong>{copy("Verify endpoint", "验证接口")}</strong><small>{copy("Check URL and credentials", "检查地址与凭据")}</small></div>
        </li>
        <li data-state={completedName ? "complete" : models.length > 0 ? "active" : "upcoming"}>
          <span>2</span>
          <div><strong>{copy("Choose models", "选择模型")}</strong><small>{copy("Connect only what you need", "只接入需要的模型")}</small></div>
        </li>
        <li data-state={completedName ? "complete" : "upcoming"}>
          <span>3</span>
          <div><strong>{copy("Finish connection", "完成接入")}</strong><small>{copy("Store the key locally", "密钥保存到本机")}</small></div>
        </li>
      </ol>

      <div className="enterprise-credential-grid">
        <label>
          <span>{copy("Base URL", "Base URL")}</span>
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
          <span>{copy("API key", "API Key")}</span>
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
          <span>{copy("Account name (optional)", "账户名称（可选）")}</span>
          <Input
            aria-label={copy("Account name", "账户名称")}
            placeholder={suggestedName}
            value={accountName}
            disabled={disabled}
            onChange={(event) => {
              setAccountName(event.target.value);
              invalidateVerification();
            }}
          />
        </label>
      </div>

      {models.length > 0 && (
        <div className="enterprise-model-picker">
          <div className="enterprise-model-picker-head">
            <strong>{copy("Choose models", "选择接入模型")}</strong>
            <span>{copy(`${selectedModels.length} selected`, `已选 ${selectedModels.length} 个`)}</span>
          </div>
          <div className="enterprise-model-options">
            {models.map((model) => (
              <label key={model}>
                <input
                  type="checkbox"
                  checked={selectedModels.includes(model)}
                  disabled={disabled}
                  onChange={(event) => setSelectedModels((current) => event.target.checked
                    ? [...current, model]
                    : current.filter((candidate) => candidate !== model))}
                />
                <span>{model}</span>
              </label>
            ))}
          </div>
          <button className="btn primary" type="button" disabled={disabled || selectedModels.length === 0} onClick={() => void connect()}>
            {working ? copy("Connecting…", "接入中…") : copy("Connect selected models", "接入所选模型")}
          </button>
        </div>
      )}

      {message && <p className="enterprise-connection-status" role="status" aria-live="polite">{message}</p>}
      <p className="enterprise-secret-note">{copy(
        "The API key is stored in the local credential store and is not shown again.",
        "API Key 仅保存到本机凭据存储，接入后不会再次显示。",
      )}</p>
    </section>
  );
}
