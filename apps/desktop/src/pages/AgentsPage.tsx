import type { ReactNode } from "react";
import { RefreshCw } from "lucide-react";
import type { AgentUiMetadataView, AgentView } from "../api";
import { AgentIcon } from "../brandIcons";
import { useLocalizedCopy } from "../components/LanguageProvider";
import { Badge } from "../components/ui/badge";
import { Button } from "../components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "../components/ui/card";
import { ScrollArea } from "../components/ui/scroll-area";

interface AgentsPageProps {
  registry: AgentUiMetadataView[];
  agents: AgentView[];
  selectedAgentId?: string;
  scanBusy: boolean;
  onRescan: () => void;
  onOpenAgent: (agentId: string) => void;
  children: ReactNode;
}

function statusCopy(status: AgentView["status"] | undefined, copy: (en: string, zh: string) => string) {
  if (status === "CONNECTED") return copy("Connected", "已接入");
  if (status === "DETECTED_VERIFIED") return copy("Ready", "可接入");
  if (status === "DETECTED_BLOCKED" || status === "INSTALLED_BROKEN") return copy("Attention", "需处理");
  if (status === "MULTIPLE_INSTALLATIONS") return copy("Choose", "待选择");
  if (status === "DETECTED_UNKNOWN") return copy("Detected", "已检测");
  return copy("Offline", "未检测");
}

function navStatusCopy(status: AgentView["status"] | undefined, copy: (en: string, zh: string) => string) {
  if (status === "CONNECTED") return copy("Managed", "接管中");
  if (status === "DETECTED_VERIFIED") return copy("Ready", "就绪");
  if (status === "MULTIPLE_INSTALLATIONS") return copy("Multiple", "多实例");
  if (status === "DETECTED_BLOCKED" || status === "INSTALLED_BROKEN") return copy("Issue", "异常");
  if (status === "DETECTED_UNKNOWN") return copy("Found", "已发现");
  return copy("Offline", "离线");
}

export default function AgentsPage({
  registry,
  agents,
  selectedAgentId,
  scanBusy,
  onRescan,
  onOpenAgent,
  children,
}: AgentsPageProps) {
  const { copy } = useLocalizedCopy();

  return (
    <div className="page-stack agents-page agent-workspace-page">
      <header className="overview-heading">
        <div>
          <span className="page-eyebrow">AGENT FLEET</span>
          <h1>{copy("Agents", "Agent 管理")}</h1>
          <p>{copy("Choose an Agent on the left, then manage connection and routing on the right.", "在左侧选择 Agent，在右侧管理接入和独立路由。")}</p>
        </div>
        <Button
          variant="outline"
          size="sm"
          data-onboarding-target="agent-rescan"
          onClick={onRescan}
          disabled={scanBusy}
        >
          <RefreshCw className={scanBusy ? "is-spinning" : ""} />
          {scanBusy ? copy("Scanning…", "扫描中…") : copy("Rescan", "重新扫描")}
        </Button>
      </header>

      <div className="agent-master-detail">
        <Card className="agent-master-list-card">
          <CardHeader>
            <div>
              <span className="page-eyebrow">AGENTS</span>
              <CardTitle><h2>{copy("Local clients", "本机客户端")}</h2></CardTitle>
            </div>
            <Badge variant="outline">{registry.length}</Badge>
          </CardHeader>
          <CardContent>
            <ScrollArea className="agent-master-scroll">
              <nav
                className="agent-master-nav"
                aria-label={copy("Agent list", "Agent 列表")}
                data-onboarding-target="agent-list"
              >
                {registry.map((metadata) => {
                  const agent = agents.find((candidate) => candidate.metadata.agent_id === metadata.agent_id);
                  const selected = metadata.agent_id === selectedAgentId;
                  return (
                    <Button
                      key={metadata.agent_id}
                      className="agent-master-item"
                      variant="ghost"
                      type="button"
                      aria-label={metadata.display_name}
                      title={`${metadata.display_name} · ${statusCopy(agent?.status, copy)}`}
                      aria-current={selected ? "page" : undefined}
                      onClick={() => onOpenAgent(metadata.agent_id)}
                    >
                      <span className="agent-master-icon" aria-hidden="true">
                        <AgentIcon id={metadata.agent_id} fallback={metadata.nav_mark ?? metadata.display_name.slice(0, 1)} size={40} />
                      </span>
                      <span><strong>{metadata.display_name}</strong><small>{metadata.agent_id}</small></span>
                      <Badge variant={agent?.status === "CONNECTED" ? "default" : "outline"}>{navStatusCopy(agent?.status, copy)}</Badge>
                    </Button>
                  );
                })}
              </nav>
            </ScrollArea>
          </CardContent>
        </Card>

        <section className="agent-master-content" aria-label={copy("Selected Agent configuration", "当前 Agent 配置")}>
          {children}
        </section>
      </div>
    </div>
  );
}
