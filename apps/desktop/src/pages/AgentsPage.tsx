import type { ReactNode } from "react";
import { RefreshCw } from "lucide-react";
import type { AgentUiMetadataView, AgentView } from "../api";
import { AgentIcon } from "../brandIcons";
import { useLocalizedCopy } from "../components/LanguageProvider";
import TokenStationMark from "../components/TokenStationMark";
import { Badge } from "../components/ui/badge";
import { Button } from "../components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "../components/ui/card";
import { ScrollArea } from "../components/ui/scroll-area";
import { cn } from "../lib/utils";

interface AgentsPageProps {
  registry: AgentUiMetadataView[];
  agents: AgentView[];
  selectedAgentId?: string;
  homeSelected: boolean;
  scanBusy: boolean;
  onOpenHome: () => void;
  onOpenAgent: (agentId: string) => void;
  onRescan: () => void;
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
  return copy("Not found", "未检测");
}

export default function AgentsPage({
  registry,
  agents,
  selectedAgentId,
  homeSelected,
  scanBusy,
  onOpenHome,
  onOpenAgent,
  onRescan,
  children,
}: AgentsPageProps) {
  const { copy } = useLocalizedCopy();

  return (
    <div className="page-stack agents-page agent-workspace-page">
      <header className="overview-heading">
        <div>
          <h1>{copy("Home", "主页")}</h1>
          <p>{copy("Choose global routing or a local client, then configure it on the right.", "选择全局路由或本机客户端，在右侧完成配置。")}</p>
        </div>
      </header>

      <div className="agent-master-detail">
        <Card
          className="agent-master-list-card"
          role="region"
          aria-label={copy("Agent selector", "客户端选择列表")}
          data-onboarding-target="agent-list"
        >
          <CardHeader>
            <div>
              <CardTitle><h2>{copy("Local clients", "本机客户端")}</h2></CardTitle>
            </div>
            <div className="agent-master-actions">
              <Badge variant="outline">{registry.length}</Badge>
              <Button
                variant="outline"
                size="sm"
                type="button"
                data-onboarding-target="agent-rescan"
                onClick={onRescan}
                disabled={scanBusy}
              >
                <RefreshCw
                  data-icon="inline-start"
                  className={cn(scanBusy && "is-spinning")}
                  aria-hidden="true"
                />
                {scanBusy ? copy("Scanning…", "扫描中…") : copy("Rescan", "重新扫描")}
              </Button>
            </div>
          </CardHeader>
          <CardContent>
            <ScrollArea className="agent-master-scroll">
              <nav
                className="agent-master-nav"
                aria-label={copy("Agent list", "客户端列表")}
              >
                <Button
                  className="agent-master-item agent-master-home"
                  variant="ghost"
                  type="button"
                  aria-label={copy("Global routing", "全局路由")}
                  title={copy("Global routing - fixed first row", "全局路由 - 固定首行")}
                  aria-current={homeSelected ? "page" : undefined}
                  data-onboarding-target="routing"
                  onClick={onOpenHome}
                >
                  <span className="agent-master-icon global-route-mark" aria-hidden="true">
                    <TokenStationMark size={36} />
                  </span>
                  <span className="agent-master-copy"><strong>{copy("Global routing", "全局路由")}</strong></span>
                  <Badge variant={homeSelected ? "default" : "outline"}>{copy("Default", "默认")}</Badge>
                </Button>
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
                      data-onboarding-target="agent-entry"
                      aria-current={selected ? "page" : undefined}
                      onClick={() => onOpenAgent(metadata.agent_id)}
                    >
                      <span className="agent-master-icon" aria-hidden="true">
                        <AgentIcon id={metadata.agent_id} fallback={metadata.nav_mark ?? metadata.display_name.slice(0, 1)} size={40} />
                      </span>
                      <span className="agent-master-copy"><strong>{metadata.display_name}</strong></span>
                      <Badge variant={agent?.status === "CONNECTED" ? "default" : "outline"}>{navStatusCopy(agent?.status, copy)}</Badge>
                    </Button>
                  );
                })}
              </nav>
            </ScrollArea>
          </CardContent>
        </Card>

        <section className="agent-master-content" aria-label={copy("Selected Home configuration", "当前主页配置")}>
          {children}
        </section>
      </div>
    </div>
  );
}
