import { useState, type ReactNode } from "react";
import { Building2, ChevronDown, RefreshCw } from "lucide-react";
import type { AgentUiMetadataView, AgentView } from "../api";
import { AgentIcon } from "../brandIcons";
import { useLocalizedCopy, type LocalizedCopy } from "../components/LanguageProvider";
import TokenStationMark from "../components/TokenStationMark";
import { Badge } from "../components/ui/badge";
import { Button } from "../components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "../components/ui/card";
import { ScrollArea } from "../components/ui/scroll-area";
import { cn } from "../lib/utils";

interface AgentsPageProps {
  mode: "connections" | "routing";
  registry: AgentUiMetadataView[];
  agents: AgentView[];
  revealingAgentIds: ReadonlySet<string>;
  selectedAgentId?: string;
  homeSelected: boolean;
  enterpriseSelected?: boolean;
  scanBusy: boolean;
  onOpenHome: () => void;
  onOpenEnterprise?: () => void;
  onOpenAgent: (agentId: string) => void;
  onRescan: () => void;
  children: ReactNode;
}

function statusCopy(status: AgentView["status"] | undefined, copy: LocalizedCopy) {
  if (status === "CONNECTED") return copy("Connected", "已接入", "已接入", "接続済み");
  if (status === "DETECTED_VERIFIED") return copy("Ready", "可接入", "可接入", "接続可能");
  if (status === "DETECTED_BLOCKED" || status === "INSTALLED_BROKEN") return copy("Attention", "需处理", "注意", "注意");
  if (status === "MULTIPLE_INSTALLATIONS") return copy("Choose", "待选择", "選擇", "選択");
  if (status === "DETECTED_UNKNOWN") return copy("Detected", "已检测", "已檢測", "検出済み");
  return copy("Offline", "未检测", "未檢測", "検出されていません");
}

function navStatusCopy(status: AgentView["status"] | undefined, copy: LocalizedCopy) {
  if (status === "CONNECTED") return copy("Managed", "接管中", "接管中", "管理中");
  if (status === "DETECTED_VERIFIED") return copy("Ready", "就绪", "就緒", "準備完了");
  if (status === "MULTIPLE_INSTALLATIONS") return copy("Multiple", "多实例", "多例項", "複数インスタンス");
  if (status === "DETECTED_BLOCKED" || status === "INSTALLED_BROKEN") return copy("Issue", "异常", "異常", "異常");
  if (status === "DETECTED_UNKNOWN") return copy("Found", "已发现", "已發現", "検出済み");
  return copy("Not found", "未检测", "未檢測", "検出されていません");
}

export default function AgentsPage({
  mode,
  registry,
  agents,
  revealingAgentIds,
  selectedAgentId,
  homeSelected,
  enterpriseSelected = false,
  scanBusy,
  onOpenHome,
  onOpenEnterprise = onOpenHome,
  onOpenAgent,
  onRescan,
  children,
}: AgentsPageProps) {
  const { copy } = useLocalizedCopy();
  const connections = mode === "connections";
  const [routeListOpen, setRouteListOpen] = useState(false);
  const revealOrder = new Map(
    registry
      .filter((metadata) => revealingAgentIds.has(metadata.agent_id))
      .map((metadata, index) => [metadata.agent_id, index]),
  );

  return (
    <div className="page-stack agents-page agent-workspace-page">
      <header className="overview-heading">
        <div>
          <h1>{connections ? copy("Agent connection", "Agent 接入", "Agent 連線", "エージェント接続") : copy("Routing", "路由配置", "路由", "ルーティング")}</h1>
          <p>{connections
            ? copy("Select a detected Agent to manage its connection.", "选择一个已发现的 Agent，查看详情并管理接入。", "選擇一個偵測到的 Agent 以管理其連線。", "検出された Agent を選択して、その接続を管理します。")
            : copy("Configure global routing or open one Agent route.", "配置全局路由，或打开一个 Agent 的独立路由。", "配置全域路由，或開啟一個 Agent 的獨立路由。", "グローバルルーティングを設定するか、1 つの Agent の独立ルーティングを開きます。")}</p>
        </div>
      </header>

      <div className="agent-master-detail">
        <Card
          className="agent-master-list-card"
          size="sm"
          role="region"
          aria-label={connections ? copy("Agent selector", "Agent 选择列表", "Agent 選擇清單", "Agent 選択リスト") : copy("Routing scopes", "路由范围", "路由範圍", "ルーティングスコープ")}
          data-onboarding-target="agent-list"
        >
          <CardHeader>
            <div>
              <CardTitle><h2>{connections ? copy("Detected Agents", "发现 Agents", "檢測到的 Agent", "検出されたエージェント") : copy("Routing scopes", "路由范围", "路由範圍", "ルーティングスコープ")}</h2></CardTitle>
            </div>
            <div className="agent-master-actions">
              {connections && (
                <>
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
                    {scanBusy ? copy("Scanning…", "扫描中…", "掃描中…", "スキャン中…") : copy("Rescan", "重新扫描", "重新掃描", "再スキャン")}
                  </Button>
                </>
              )}
            </div>
          </CardHeader>
          <CardContent>
            <ScrollArea className="agent-master-scroll">
              <nav
                className="agent-master-nav"
                aria-label={connections ? copy("Detected Agent list", "发现 Agent 列表", "偵測到的 Agent 清單", "検出された Agent リスト") : copy("Routing scope list", "路由范围列表", "路由範圍清單", "ルーティングスコープリスト")}
              >
                {!connections && (
                  <div
                    className="routing-scope-global-group"
                    role="group"
                    aria-label={copy("Global routing and Agent routes", "全局路由与 Agent 路由", "全域路由與 Agent 路由", "グローバルルーティングと Agent ルーティング")}
                  >
                    <Button
                      className="agent-master-item agent-master-home"
                      variant="ghost"
                      type="button"
                      aria-label={copy("Global routing", "全局路由", "全域路由", "グローバルルーティング")}
                      title={copy("Global routing", "全局路由", "全域路由", "グローバルルーティング")}
                      aria-current={homeSelected ? "page" : undefined}
                      data-onboarding-target="routing"
                      onClick={onOpenHome}
                    >
                      <span className="agent-master-icon global-route-mark" aria-hidden="true">
                        <TokenStationMark size={36} />
                      </span>
                      <span className="agent-master-copy"><strong>{copy("Global routing", "全局路由", "全域路由", "グローバルルーティング")}</strong></span>
                      <Badge variant={homeSelected ? "secondary" : "ghost"}>{homeSelected
                        ? copy("Current", "当前", "當前", "表示中")
                        : copy("Switch", "切换", "切換", "切替")}</Badge>
                    </Button>
                    <div className="agent-route-disclosure">
                      <Button
                        className="agent-route-disclosure-trigger"
                        variant="ghost"
                        type="button"
                        aria-label={copy("Agent routes", "Agent 路由", "Agent 路由", "Agent ルーティング")}
                        aria-expanded={routeListOpen}
                        aria-controls="agent-route-list"
                        onClick={() => setRouteListOpen((open) => !open)}
                      >
                        <span className="agent-route-disclosure-label">
                          <span>{copy("Agent routes", "Agent 路由", "Agent 路由", "Agent ルーティング")}</span>
                          <Badge variant="outline">{registry.length}</Badge>
                        </span>
                        <ChevronDown data-icon="inline-end" aria-hidden="true" />
                      </Button>
                      {routeListOpen && (
                        <div id="agent-route-list" className="agent-route-list">
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
                                  <AgentIcon id={metadata.agent_id} fallback={metadata.nav_mark ?? metadata.display_name.slice(0, 1)} size={28} />
                                </span>
                                <span className="agent-master-copy"><strong>{metadata.display_name}</strong></span>
                                <Badge variant={agent?.status === "CONNECTED" ? "secondary" : "ghost"}>{navStatusCopy(agent?.status, copy)}</Badge>
                              </Button>
                            );
                          })}
                        </div>
                      )}
                    </div>
                  </div>
                )}
                {connections && registry.map((metadata) => {
                  const agent = agents.find((candidate) => candidate.metadata.agent_id === metadata.agent_id);
                  const selected = metadata.agent_id === selectedAgentId;
                  const revealIndex = revealOrder.get(metadata.agent_id);
                  return (
                    <Button
                      key={metadata.agent_id}
                      className={cn(
                        "agent-master-item",
                        revealIndex !== undefined && "agent-master-item-revealing",
                      )}
                      style={revealIndex === undefined ? undefined : {
                        animationDelay: `${Math.min(revealIndex * 50, 300)}ms`,
                      }}
                      variant="ghost"
                      type="button"
                      aria-label={metadata.display_name}
                      title={`${metadata.display_name} · ${statusCopy(agent?.status, copy)}`}
                      data-onboarding-target="agent-entry"
                      aria-current={selected ? "page" : undefined}
                      onClick={() => onOpenAgent(metadata.agent_id)}
                    >
                      <span className="agent-master-icon" aria-hidden="true">
                        <AgentIcon id={metadata.agent_id} fallback={metadata.nav_mark ?? metadata.display_name.slice(0, 1)} size={28} />
                      </span>
                      <span className="agent-master-copy"><strong>{metadata.display_name}</strong></span>
                      <Badge variant={agent?.status === "CONNECTED" ? "secondary" : "ghost"}>{navStatusCopy(agent?.status, copy)}</Badge>
                    </Button>
                  );
                })}
                {!connections && (
                  <Button
                    className="agent-master-item agent-master-enterprise"
                    variant="ghost"
                    type="button"
                    aria-label={copy("Enterprise routing", "企业路由", "企業路由", "企業ルーティング")}
                    title={copy(
                      "Connect and use a server-managed routing endpoint",
                      "接入并使用由企业服务管理模型与策略的路由端点",
                      "接入並使用由企業服務管理模型與策略的路由端點",
                      "企業サービスがモデルとポリシーを管理するルーティングエンドポイントに接続して使用します",
                    )}
                    aria-current={enterpriseSelected ? "page" : undefined}
                    onClick={onOpenEnterprise}
                  >
                    <span className="agent-master-icon enterprise-route-mark" aria-hidden="true">
                      <Building2 />
                    </span>
                    <span className="agent-master-copy"><strong>{copy("Enterprise routing", "企业路由", "企業路由", "企業ルーティング")}</strong></span>
                    <Badge variant={enterpriseSelected ? "secondary" : "ghost"}>{enterpriseSelected
                      ? copy("Current", "当前", "當前", "表示中")
                      : copy("Switch", "切换", "切換", "切替")}</Badge>
                  </Button>
                )}
              </nav>
            </ScrollArea>
          </CardContent>
        </Card>

        <section className="agent-master-content" aria-label={connections ? copy("Selected Agent details", "当前 Agent 详情", "當前 Agent 詳細", "現在の Agent の詳細") : copy("Selected routing configuration", "当前路由配置", "當前路由配置", "現在のルーティング設定")}>
          {children}
        </section>
      </div>
    </div>
  );
}
