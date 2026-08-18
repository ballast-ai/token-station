import { ArrowUpRight } from "lucide-react";
import { useLocalizedCopy } from "../components/LanguageProvider";
import { Button } from "../components/ui/button";
import Stats from "./Stats";

interface UsageWorkspaceProps {
  onOpenManagement: () => void;
}

export default function UsageWorkspace({ onOpenManagement }: UsageWorkspaceProps) {
  const { copy } = useLocalizedCopy();

  return (
    <div className="page-stack usage-workspace-page">
      <header className="overview-heading usage-workspace-heading page-heading-with-action">
        <div>
          <h1>{copy("Usage", "用量统计")}</h1>
          <p>{copy(
            "Review model usage, cost, and reliability. Only local request metadata is aggregated.",
            "查看模型消耗、成本与稳定性；只聚合本地请求元数据。",
          )}</p>
        </div>
        <Button className="usage-management-link" variant="outline" type="button" onClick={onOpenManagement}>
          {copy("Budget and pricing", "预算与定价")}
          <ArrowUpRight aria-hidden="true" />
        </Button>
      </header>
      <Stats embedded />
    </div>
  );
}
