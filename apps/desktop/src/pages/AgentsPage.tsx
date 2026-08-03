import { Bot, ChevronRight, RefreshCw } from "lucide-react";
import type { AgentUiMetadataView, AgentView } from "../api";
import { AgentIcon } from "../brandIcons";
import { useLocalizedCopy } from "../components/LanguageProvider";
import { Badge } from "../components/ui/badge";
import { Button } from "../components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "../components/ui/card";

interface AgentsPageProps {
  registry: AgentUiMetadataView[];
  agents: AgentView[];
  scanBusy: boolean;
  onRescan: () => void;
  onOpenAgent: (agentId: string) => void;
}

function statusCopy(status: AgentView["status"] | undefined, copy: (en: string, zh: string) => string) {
  if (status === "CONNECTED") return copy("Connected", "已接入");
  if (status === "DETECTED_VERIFIED") return copy("Ready to connect", "可接入");
  if (status === "DETECTED_BLOCKED" || status === "INSTALLED_BROKEN") return copy("Needs attention", "需要处理");
  if (status === "MULTIPLE_INSTALLATIONS") return copy("Choose installation", "选择安装位置");
  if (status === "DETECTED_UNKNOWN") return copy("Detected", "已检测");
  return copy("Not detected", "未检测");
}

export default function AgentsPage({ registry, agents, scanBusy, onRescan, onOpenAgent }: AgentsPageProps) {
  const { copy } = useLocalizedCopy();
  return (
    <div className="page-stack agents-page">
      <header className="overview-heading">
        <div>
          <span className="page-eyebrow">AGENT FLEET</span>
          <h1>{copy("Agents", "Agent 管理")}</h1>
          <p>{copy("Discover local AI clients, connect them, and manage each route independently.", "发现本机 AI 客户端，完成接入，并按 Agent 管理独立路由。")}</p>
        </div>
        <Button variant="outline" size="sm" onClick={onRescan} disabled={scanBusy}>
          <RefreshCw className={scanBusy ? "is-spinning" : ""} />
          {scanBusy ? copy("Scanning…", "扫描中…") : copy("Rescan", "重新扫描")}
        </Button>
      </header>

      <section className="agent-card-grid" aria-label={copy("Supported Agents", "支持的 Agent")}>
        {registry.map((metadata) => {
          const agent = agents.find((candidate) => candidate.metadata.agent_id === metadata.agent_id);
          const connected = agent?.status === "CONNECTED";
          const detected = Boolean(agent?.installations.length);
          return (
            <Card key={metadata.agent_id} className="agent-summary-card">
              <CardHeader>
                <span className="agent-summary-icon"><AgentIcon id={metadata.agent_id} fallback={metadata.nav_mark ?? metadata.display_name.slice(0, 1)} size={34} /></span>
                <div><CardTitle>{metadata.display_name}</CardTitle><p>{metadata.agent_id}</p></div>
                <Badge variant={connected ? "default" : detected ? "secondary" : "outline"}>{statusCopy(agent?.status, copy)}</Badge>
              </CardHeader>
              <CardContent>
                <dl>
                  <div><dt>{copy("Installations", "安装实例")}</dt><dd>{agent?.installations.length ?? 0}</dd></div>
                  <div><dt>{copy("Route", "路由")}</dt><dd>{connected ? copy("Managed", "已接管") : copy("Not managed", "未接管")}</dd></div>
                </dl>
                <Button
                  variant="ghost"
                  aria-label={metadata.display_name}
                  title={`${metadata.display_name} · ${statusCopy(agent?.status, copy)}`}
                  onClick={() => onOpenAgent(metadata.agent_id)}
                >
                  <Bot />{copy("Open Agent", "打开 Agent")}<ChevronRight />
                </Button>
              </CardContent>
            </Card>
          );
        })}
      </section>
    </div>
  );
}
