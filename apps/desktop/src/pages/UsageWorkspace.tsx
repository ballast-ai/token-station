import { useLocalizedCopy } from "../components/LanguageProvider";
import { Tabs, TabsList, TabsTrigger } from "../components/ui/tabs";
import RequestLogsPage from "./RequestLogsPage";
import Stats from "./Stats";

export type UsageSection = "overview" | "logs";

interface UsageWorkspaceProps {
  section: UsageSection;
  onSectionChange: (section: UsageSection) => void;
}

export default function UsageWorkspace({ section, onSectionChange }: UsageWorkspaceProps) {
  const { copy } = useLocalizedCopy();
  const logs = section === "logs";

  return (
    <div className="page-stack usage-workspace-page">
      <header className="overview-heading usage-workspace-heading">
        <div>
          <span className="page-eyebrow">LOCAL RECEIPT LEDGER</span>
          <h1>{logs ? copy("Request logs", "请求日志") : copy("Usage", "用量统计")}</h1>
          <p>{logs
            ? copy("Inspect routing outcomes, failures, and locally retained plaintext bodies.", "查看路由结果、失败原因和本地保留的明文正文。")
            : copy("Review model usage, cost, and reliability. Only local request metadata is aggregated.", "查看模型消耗、成本与稳定性；只聚合本地请求元数据。")}</p>
        </div>
      </header>

      <Tabs
        className="usage-workspace-tabs"
        value={section}
        onValueChange={(value) => onSectionChange(value as UsageSection)}
      >
        <TabsList aria-label={copy("Usage views", "用量视图")}>
          <TabsTrigger value="overview">{copy("Usage overview", "用量概览")}</TabsTrigger>
          <TabsTrigger value="logs">{copy("Request logs", "请求日志")}</TabsTrigger>
        </TabsList>
      </Tabs>

      {logs ? <RequestLogsPage embedded /> : <Stats embedded />}
    </div>
  );
}
