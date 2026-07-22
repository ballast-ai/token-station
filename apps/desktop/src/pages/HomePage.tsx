import {
  saveHomeRouteAsProfile,
  type AgentRouteView,
  type AgentUiMetadataView,
  type AgentView,
  type ProviderView,
  type StateView,
  type TierSlot,
  type TierView,
} from "../api";
import TierRouteEditor from "../components/TierRouteEditor";
import ProviderList from "../components/ProviderList";
import RecentRequests from "../components/RecentRequests";

interface HomePageProps {
  providers: ProviderView[];
  tiers: Record<TierSlot, TierView>;
  agentRoutes: Record<string, AgentRouteView>;
  registry: AgentUiMetadataView[];
  agents: AgentView[];
  serveRunning: boolean;
  dirty: boolean;
  applied: boolean;
  busy: boolean;
  configError: string | null;
  onTierChange: (slot: TierSlot, upstream: string | null, model: string | null) => void;
  onSave: () => void;
  onApplyAll: () => void;
  onOpenAgent: (agentId: string) => void;
  onRemoveProvider: (name: string) => void;
  onStateChange: (state: StateView, message: string) => void;
}

export default function HomePage({
  providers,
  tiers,
  agentRoutes,
  registry,
  agents,
  serveRunning,
  dirty,
  applied,
  busy,
  configError,
  onTierChange,
  onSave,
  onApplyAll,
  onOpenAgent,
  onRemoveProvider,
  onStateChange,
}: HomePageProps) {
  const scanned = new Map(agents.map((agent) => [agent.metadata.agent_id, agent]));
  return (
    <div className="page-stack home-page">
      <header className="page-title-row">
        <div>
          <span className="eyebrow">DEFAULT ROUTE</span>
          <h1>主页路由</h1>
          <p>这套三档配置是所有 Agent 的默认值。独立路由只覆盖对应 Agent。</p>
        </div>
      </header>

      <section className="agent-overview" aria-label="Agent 路由摘要">
        {registry.map((metadata) => {
          const agent = scanned.get(metadata.agent_id);
          const route = agentRoutes[metadata.agent_id];
          const connected = agent?.status === "CONNECTED";
          return (
            <button key={metadata.agent_id} type="button" onClick={() => onOpenAgent(metadata.agent_id)}>
              <span className={`overview-signal ${connected ? "connected" : agent?.installations.length ? "detected" : ""}`} />
              <strong>{metadata.display_name.replace(" Agent", "")}</strong>
              <small>{connected ? "已接入" : agent?.installations.length ? "已发现" : "未发现"}</small>
              <em>{route?.mode === "custom" ? "独立路由" : "跟随主页"}</em>
            </button>
          );
        })}
      </section>

      <section className="panel route-panel">
        <div className="panel-head split-heading">
          <div>
            <span className="eyebrow">SMART ROUTING · 3 TIERS</span>
            <h2>智能路由 · 三档</h2>
            <p className="sub">请求按复杂度自动落档；你只选择每档的供应商和模型。</p>
          </div>
          <span className="default-route-chip">全局默认</span>
        </div>

        <TierRouteEditor
          tiers={tiers}
          providers={providers}
          disabled={busy}
          onTierChange={onTierChange}
        />

        <footer className="panel-foot route-actions">
          <button className="btn primary" type="button" disabled={busy} onClick={onSave}>保存并应用</button>
          <button className="btn" type="button" disabled={busy} onClick={onApplyAll}>应用到全部 Agent</button>
          <button
            className="btn"
            type="button"
            disabled={busy || Boolean(configError)}
            title="把当前三档存成命名策略组,供多个 Agent 挂载共用"
            onClick={() => {
              const name = window.prompt("策略组名称(供多个 Agent 挂载共用):");
              if (name?.trim()) {
                void saveHomeRouteAsProfile(name.trim()).then((next) =>
                  onStateChange(next, `已另存为策略组「${name.trim()}」`),
                );
              }
            }}
          >
            另存为策略组
          </button>
          <span className={`save-state ${dirty ? "dirty" : !applied ? "unapplied" : "clean"}`}>
            {dirty
              ? "● 有未保存更改"
              : serveRunning && !applied
                ? "▲ 已保存，尚未应用 · 重启代理后生效"
                : serveRunning
                  ? "✓ 运行中 · 已应用"
                  : "✓ 已保存"}
          </span>
          {providers.length === 0 && <span className="foot-hint">请先添加供应商，再配置三档。</span>}
          {providers.length > 0 && configError && <span className="foot-hint">还有档位未完成，保存时会进行完整校验。</span>}
        </footer>
      </section>

      <ProviderList
        providers={providers}
        serveRunning={serveRunning}
        busy={busy}
        onRemove={onRemoveProvider}
        onStateChange={onStateChange}
      />

      <RecentRequests />
    </div>
  );
}
