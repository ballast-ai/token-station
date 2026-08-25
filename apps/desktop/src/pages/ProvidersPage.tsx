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
  onRemove: (name: string) => Promise<boolean>;
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
          <h1>{copy("Models", "模型", "模型", "モデル")}</h1>
          <p>{copy("Manage models first, with each delivery provider shown beside it.", "以模型为主进行管理，并在模型后标明实际供应商。", "以模型為主進行管理，並在模型後標明實際供應商。", "モデルを主として管理し、モデルの後ろに実際のプロバイダーを記載します。")}</p>
        </div>
        <Button
          className="providers-add-button"
          data-onboarding-target="add-provider"
          type="button"
          onClick={props.onAddProvider}
        >
          <Plus aria-hidden="true" />
          {copy("Add model", "添加模型", "新增模型", "モデルを追加")}
        </Button>
      </header>
      <ProviderList {...props} />
    </div>
  );
}
