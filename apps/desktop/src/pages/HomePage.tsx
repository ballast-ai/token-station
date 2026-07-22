import type {
  AgentRouteView,
  AgentUiMetadataView,
  AgentView,
  ProviderView,
  StateView,
  TierSlot,
  TierView,
} from "../api";
import TierRouteEditor from "../components/TierRouteEditor";
import ProviderList from "../components/ProviderList";

interface HomePageProps {
  providers: ProviderView[];
  deletedProviders: string[];
  providerRecoveryError: string | null;
  tiers: Record<TierSlot, TierView>;
  agentRoutes: Record<string, AgentRouteView>;
  registry: AgentUiMetadataView[];
  agents: AgentView[];
  serveRunning: boolean;
  busy: boolean;
  configError: string | null;
  saveStatus: string;
  onTierChange: (slot: TierSlot, upstream: string | null, model: string | null) => void;
  onSave: () => void;
  onApplyAll: () => void;
  onOpenAgent: (agentId: string) => void;
  onRemoveProvider: (name: string) => void;
  onRestoreProvider: (name: string) => void;
  onStateChange: (state: StateView, message: string) => void;
}

export default function HomePage({
  providers,
  deletedProviders,
  providerRecoveryError,
  tiers,
  agentRoutes,
  registry,
  agents,
  serveRunning,
  busy,
  configError,
  saveStatus,
  onTierChange,
  onSave,
  onApplyAll,
  onOpenAgent,
  onRemoveProvider,
  onRestoreProvider,
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
          <span className="foot-hint" data-testid="config-save-status">{saveStatus}</span>
          {providers.length === 0 && <span className="foot-hint">请先添加供应商，再配置三档。</span>}
          {providers.length > 0 && configError && <span className="foot-hint">还有档位未完成，保存时会进行完整校验。</span>}
        </footer>
      </section>

      <ProviderList
        providers={providers}
        deletedProviders={deletedProviders}
        recoveryError={providerRecoveryError}
        serveRunning={serveRunning}
        busy={busy}
        onRemove={onRemoveProvider}
        onRestore={onRestoreProvider}
        onStateChange={onStateChange}
      />
    </div>
  );
}
