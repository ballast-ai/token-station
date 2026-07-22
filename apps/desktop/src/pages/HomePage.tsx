import { useState } from "react";
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
import TierKeywords from "../components/TierKeywords";
import ProviderList from "../components/ProviderList";
import RecentRequests from "../components/RecentRequests";

interface HomePageProps {
  providers: ProviderView[];
  tiers: Record<TierSlot, TierView>;
  keywords: Record<TierSlot, string[]>;
  agentRoutes: Record<string, AgentRouteView>;
  registry: AgentUiMetadataView[];
  agents: AgentView[];
  serveRunning: boolean;
  dirty: boolean;
  applied: boolean;
  busy: boolean;
  configError: string | null;
  onTierChange: (slot: TierSlot, upstream: string | null, model: string | null) => void;
  onAddKeyword: (slot: TierSlot, keyword: string) => void;
  onRemoveKeyword: (slot: TierSlot, keyword: string) => void;
  onSave: () => void;
  onApplyAll: () => void;
  onOpenAgent: (agentId: string) => void;
  onRemoveProvider: (name: string) => void;
  onStateChange: (state: StateView, message: string) => void;
}

export default function HomePage({
  providers,
  tiers,
  keywords,
  agentRoutes,
  registry,
  agents,
  serveRunning,
  dirty,
  applied,
  busy,
  configError,
  onTierChange,
  onAddKeyword,
  onRemoveKeyword,
  onSave,
  onApplyAll,
  onOpenAgent,
  onRemoveProvider,
  onStateChange,
}: HomePageProps) {
  const tierConfigured: Record<TierSlot, boolean> = {
    high: Boolean(tiers.high?.upstream && tiers.high?.model),
    mid: Boolean(tiers.mid?.upstream && tiers.mid?.model),
    low: Boolean(tiers.low?.upstream && tiers.low?.model),
  };
  const scanned = new Map(agents.map((agent) => [agent.metadata.agent_id, agent]));
  const [namingProfile, setNamingProfile] = useState(false);
  const [profileName, setProfileName] = useState("");

  const [profileError, setProfileError] = useState("");
  const confirmSaveProfile = () => {
    const name = profileName.trim();
    if (!name) return;
    void saveHomeRouteAsProfile(name)
      .then((next) => {
        onStateChange(next, `已另存为策略组「${name}」`);
        setNamingProfile(false);
        setProfileName("");
        setProfileError("");
      })
      .catch((caught) => setProfileError(String(caught)));
  };

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
          {namingProfile ? (
            <span className="profile-save-inline">
              <input
                className="input"
                autoFocus
                placeholder="策略组名称"
                value={profileName}
                disabled={busy}
                onChange={(event) => setProfileName(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === "Enter") confirmSaveProfile();
                  if (event.key === "Escape") setNamingProfile(false);
                }}
              />
              <button className="btn primary" type="button" disabled={busy || !profileName.trim()} onClick={confirmSaveProfile}>存</button>
              <button className="btn" type="button" disabled={busy} onClick={() => { setNamingProfile(false); setProfileError(""); }}>取消</button>
            </span>
          ) : (
            <button
              className="btn"
              type="button"
              disabled={busy || Boolean(configError)}
              title="把当前三档存成命名策略组,供多个 Agent 挂载共用"
              onClick={() => { setNamingProfile(true); setProfileError(""); }}
            >
              另存为策略组
            </button>
          )}
          {profileError && <span className="foot-hint error-text">{profileError}</span>}
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

      <section className="panel keyword-panel">
        <div className="panel-head split-heading">
          <div>
            <span className="eyebrow">KEYWORD OVERRIDE · YOU'RE IN CONTROL</span>
            <h2>关键词路由 · 你说了算</h2>
            <p className="sub">
              自动分档不称心？<strong>你来定</strong>:给某一档加个关键词,以后请求里
              只要出现它,就<strong>钉在这一档</strong>、压过自动判断。加完按上方「保存并应用」生效。
            </p>
          </div>
          <span className="default-route-chip">最高优先级</span>
        </div>

        <TierKeywords
          keywords={keywords}
          configured={tierConfigured}
          disabled={busy}
          onAdd={onAddKeyword}
          onRemove={onRemoveKeyword}
        />
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
