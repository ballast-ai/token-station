import type { ProviderView, StateView } from "../api";
import { Plus } from "lucide-react";
import ProviderList from "../components/ProviderList";
import { useLocalizedCopy } from "../components/LanguageProvider";
import { Button } from "../components/ui/button";

interface ProvidersPageProps {
  providers: ProviderView[];
  deletedProviders: string[];
  recoveryError: string | null;
  serveRunning: boolean;
  busy: boolean;
  onRemove: (name: string) => void;
  onRestore: (name: string) => void;
  onStateChange: (state: StateView) => void;
  onAddProvider: () => void;
}

export default function ProvidersPage(props: ProvidersPageProps) {
  const { copy } = useLocalizedCopy();
  return (
    <div className="page-stack providers-page">
      <header className="overview-heading page-heading-with-action">
        <div>
          <h1>{copy("Providers", "供应商管理")}</h1>
          <p>{copy("Credentials, endpoints, and model catalogs shared by global and Agent routes.", "集中维护凭据、端点和模型目录，供全局路由和各客户端共用。")}</p>
        </div>
        <Button
          className="providers-add-button"
          data-onboarding-target="add-provider"
          type="button"
          onClick={props.onAddProvider}
        >
          <Plus aria-hidden="true" />
          {copy("Add provider", "添加供应商")}
        </Button>
      </header>
      <ProviderList {...props} />
    </div>
  );
}
