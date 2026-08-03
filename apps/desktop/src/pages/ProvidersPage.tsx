import type { ProviderView, StateView } from "../api";
import ProviderList from "../components/ProviderList";
import { useLocalizedCopy } from "../components/LanguageProvider";

interface ProvidersPageProps {
  providers: ProviderView[];
  deletedProviders: string[];
  recoveryError: string | null;
  serveRunning: boolean;
  busy: boolean;
  onRemove: (name: string) => void;
  onRestore: (name: string) => void;
  onStateChange: (state: StateView, message: string) => void;
}

export default function ProvidersPage(props: ProvidersPageProps) {
  const { copy } = useLocalizedCopy();
  return (
    <div className="page-stack providers-page">
      <header className="overview-heading">
        <div>
          <span className="page-eyebrow">UPSTREAM CATALOG</span>
          <h1>{copy("Providers", "供应商管理")}</h1>
          <p>{copy("Credentials, endpoints, and model catalogs shared by global and Agent routes.", "集中维护凭据、端点和模型目录，供全局与 Agent 路由共用。")}</p>
        </div>
      </header>
      <ProviderList {...props} />
    </div>
  );
}
