import { useState } from "react";
import type { ModelDiscoveryView, ProviderView, StateView } from "../api";
import { Building2, Plus } from "lucide-react";
import ProviderList from "../components/ProviderList";
import EnterpriseConnectionPanel, {
  ENTERPRISE_PROVIDER_ID,
  type EnterpriseConnectionInput,
} from "../components/EnterpriseConnectionPanel";
import { useLocalizedCopy } from "../components/LanguageProvider";
import { Button } from "../components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "../components/ui/dialog";

interface ProvidersPageProps {
  providers: ProviderView[];
  deletedProviders: string[];
  recoveryError: string | null;
  serveRunning: boolean;
  busy: boolean;
  onRemove: (name: string) => Promise<boolean>;
  onRestore: (name: string) => void;
  onStateChange: (state: StateView) => void;
  onAddProvider: () => void;
  onVerifyEnterprise: (connection: Pick<EnterpriseConnectionInput, "name" | "baseUrl" | "apiKey">) => Promise<ModelDiscoveryView>;
  onConnectEnterprise: (connection: EnterpriseConnectionInput) => boolean | Promise<boolean>;
}

export default function ProvidersPage(props: ProvidersPageProps) {
  const { copy } = useLocalizedCopy();
  const [enterpriseOpen, setEnterpriseOpen] = useState(false);
  const existingEnterprise = props.providers.find((provider) => (
    provider.managed_route && provider.name === ENTERPRISE_PROVIDER_ID
  )) ?? null;
  return (
    <div className="page-stack providers-page">
      <header className="overview-heading page-heading-with-action">
        <div>
          <h1>{copy("Models", "模型", "模型", "モデル")}</h1>
          <p>{copy("Manage models first, with each delivery provider shown beside it.", "以模型为主进行管理，并在模型后标明实际供应商。", "以模型為主進行管理，並在模型後標明實際供應商。", "モデルを主として管理し、モデルの後ろに実際のプロバイダーを記載します。")}</p>
        </div>
        <div className="providers-heading-actions">
          <Button className="providers-enterprise-button" variant="outline" type="button" onClick={() => setEnterpriseOpen(true)}>
            <Building2 aria-hidden="true" />
            {copy("Enterprise routing", "企业路由", "企業路由", "企業ルーティング")}
          </Button>
          <Button
            className="providers-add-button"
            data-onboarding-target="add-provider"
            type="button"
            onClick={props.onAddProvider}
          >
            <Plus aria-hidden="true" />
            {copy("Add model", "添加模型", "新增模型", "モデルを追加")}
          </Button>
        </div>
      </header>
      <ProviderList {...props} />
      <Dialog open={enterpriseOpen} onOpenChange={setEnterpriseOpen}>
        <DialogContent className="enterprise-route-dialog" closeLabel={copy("Close", "关闭", "關閉", "閉じる")}>
          <DialogHeader>
            <DialogTitle>{copy("Add enterprise route", "添加企业路由", "新增企業路由", "企業ルートを追加")}</DialogTitle>
            <DialogDescription>{copy(
              "Connect a Token-station endpoint, verify its credential, and select one model.",
              "连接 Token-station 地址，验证凭据并选择一个模型。",
              "連接 Token-station 端點，驗證憑據並選擇一個模型。",
              "Token-station エンドポイントに接続し、認証情報を検証してモデルを1つ選択します。",
            )}</DialogDescription>
          </DialogHeader>
          <EnterpriseConnectionPanel
            existingProvider={existingEnterprise}
            busy={props.busy}
            onVerify={props.onVerifyEnterprise}
            onConnect={async (connection) => {
              const connected = await props.onConnectEnterprise(connection);
              if (connected) setEnterpriseOpen(false);
              return connected;
            }}
          />
        </DialogContent>
      </Dialog>
    </div>
  );
}
