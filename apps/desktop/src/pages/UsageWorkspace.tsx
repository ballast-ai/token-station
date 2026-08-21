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
          <h1>{copy("Usage", "用量统计", "用量", "使用状況")}</h1>
          <p>{copy(
            "Review model usage, cost, and reliability. Only local request metadata is aggregated.",
            "查看模型消耗、成本与稳定性；只聚合本地请求元数据。", "檢視模型消耗、成本與穩定性；只聚合本地請求後設資料。", "モデルの使用量、コスト、信頼性を確認；ローカルリクエストのメタデータのみを集約します。"
          )}</p>
        </div>
        <Button className="usage-management-link" variant="outline" type="button" onClick={onOpenManagement}>
          {copy("Budget and pricing", "预算与定价", "預算與定價", "予算と価格")}
          <ArrowUpRight aria-hidden="true" />
        </Button>
      </header>
      <Stats embedded />
    </div>
  );
}
