import { useState } from "react";
import {
  deleteProfile,
  saveHomeRouteAsProfile,
  type ProviderView,
  type StateView,
  type TierSlot,
  type TierView,
} from "../api";
import TierRouteEditor from "../components/TierRouteEditor";
import TierKeywords from "../components/TierKeywords";
import ProviderList from "../components/ProviderList";
import RecentReceipts from "../components/RecentReceipts";

interface HomePageProps {
  providers: ProviderView[];
  deletedProviders: string[];
  providerRecoveryError: string | null;
  tiers: Record<TierSlot, TierView>;
  keywords: Record<TierSlot, string[]>;
  profiles: string[];
  serveRunning: boolean;
  busy: boolean;
  applying: boolean;
  configError: string | null;
  saveStatus: string;
  localOnly: boolean;
  allowCloudFallback: boolean;
  onSetLocalRouting: (localOnly: boolean, allowCloudFallback: boolean) => void;
  onTierChange: (slot: TierSlot, upstream: string | null, model: string | null) => void;
  onSyncTiers: () => void;
  onAddKeyword: (slot: TierSlot, keyword: string) => void;
  onRemoveKeyword: (slot: TierSlot, keyword: string) => void;
  onSave: () => void;
  onApplyAll: () => void;
  onRemoveProvider: (name: string) => void;
  onRestoreProvider: (name: string) => void;
  onStateChange: (state: StateView, message: string) => void;
}

function errorText(error: unknown): string {
  if (typeof error === "string") return error;
  if (error && typeof error === "object" && "message" in error) {
    return String((error as { message: unknown }).message);
  }
  return String(error);
}

export default function HomePage({
  providers,
  deletedProviders,
  providerRecoveryError,
  tiers,
  keywords,
  profiles,
  serveRunning,
  busy,
  applying,
  configError,
  saveStatus,
  localOnly,
  allowCloudFallback,
  onSetLocalRouting,
  onTierChange,
  onSyncTiers,
  onAddKeyword,
  onRemoveKeyword,
  onSave,
  onApplyAll,
  onRemoveProvider,
  onRestoreProvider,
  onStateChange,
}: HomePageProps) {
  const tierConfigured: Record<TierSlot, boolean> = {
    high: Boolean(tiers.high?.upstream && tiers.high?.model),
    mid: Boolean(tiers.mid?.upstream && tiers.mid?.model),
    low: Boolean(tiers.low?.upstream && tiers.low?.model),
  };
  const hasLocalProvider = providers.some((provider) => provider.local);
  const [profileName, setProfileName] = useState("");
  const [profileBusy, setProfileBusy] = useState(false);
  const [profileError, setProfileError] = useState("");
  const [profileOpen, setProfileOpen] = useState(false);

  const saveProfile = async () => {
    const name = profileName.trim();
    if (!name || profileBusy) return;
    setProfileBusy(true);
    setProfileError("");
    try {
      const next = await saveHomeRouteAsProfile(name);
      setProfileName("");
      setProfileOpen(false);
      onStateChange(next, `策略组「${name}」已加入草稿，请保存并应用`);
    } catch (caught) {
      setProfileError(errorText(caught));
    } finally {
      setProfileBusy(false);
    }
  };

  const removeProfile = async (name: string) => {
    if (profileBusy) return;
    setProfileBusy(true);
    setProfileError("");
    try {
      const next = await deleteProfile(name);
      onStateChange(next, `策略组「${name}」已从草稿删除，请保存并应用`);
    } catch (caught) {
      setProfileError(errorText(caught));
    } finally {
      setProfileBusy(false);
    }
  };
  return (
    <div className="page-stack home-page">
      <header className="page-title-row">
        <div>
          <h1>主页路由</h1>
          <p>这套三档配置是所有 Agent 的默认值。独立路由只覆盖对应 Agent。</p>
        </div>
      </header>

      <section className="panel route-panel">
        <div className="panel-head split-heading">
          <div>
            <h2>智能路由</h2>
            <p className="sub">根据任务复杂度选择不同模型。</p>
          </div>
          <div className="route-heading-actions">
            {profiles.length > 0 && <span className="count-badge">{profiles.length} 个策略</span>}
            <button
              className="btn quiet"
              type="button"
              aria-expanded={profileOpen}
              disabled={busy || profileBusy}
              onClick={() => setProfileOpen((current) => !current)}
            >
              {profileOpen ? "收起" : "存为策略"}
            </button>
          </div>
        </div>

        <TierRouteEditor
          tiers={tiers}
          providers={providers}
          disabled={busy}
          onTierChange={onTierChange}
          onSyncTiers={onSyncTiers}
        />

        {profileOpen && <div className="profile-manager compact-profile-manager">
          <div className="profile-create-row">
            <input
              className="input"
              aria-label="策略组名称"
              value={profileName}
              maxLength={80}
              placeholder="例如：日常开发"
              disabled={busy || profileBusy}
              onChange={(event) => setProfileName(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter") void saveProfile();
              }}
            />
            <button className="btn primary" type="button" disabled={busy || profileBusy || !profileName.trim()} onClick={() => void saveProfile()}>保存策略</button>
            <button className="btn quiet" type="button" disabled={profileBusy} onClick={() => setProfileOpen(false)}>取消</button>
          </div>
          {profiles.length > 0 && (
            <div className="profile-list" aria-label="已有策略组">
              {profiles.map((profile) => (
                <span key={profile}>
                  {profile}
                  <button type="button" aria-label={`删除策略组 ${profile}`} disabled={busy || profileBusy} onClick={() => void removeProfile(profile)}>×</button>
                </span>
              ))}
            </div>
          )}
          {profileError && <span className="foot-hint error-text">{profileError}</span>}
        </div>}

        <footer className="panel-foot route-actions">
          <div className="route-status-copy">
            <span className="foot-hint" data-testid="config-save-status">{saveStatus}</span>
            {providers.length === 0 && <span className="foot-hint">请先添加供应商，再配置三档。</span>}
            {providers.length > 0 && configError && <span className="foot-hint">还有档位未完成，保存时会进行完整校验。</span>}
          </div>
          <div className="route-action-buttons">
            <button className="btn" type="button" disabled={busy || applying} onClick={onApplyAll}>应用到全部 Agent</button>
            <button className="btn primary" type="button" disabled={busy || applying} onClick={onSave}>
              {applying ? "应用中…" : "保存并应用"}
            </button>
          </div>
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

      <section className="panel local-routing-panel">
        <div className="panel-head split-heading">
          <div>
            <span className="eyebrow">LOCAL-ONLY · DATA STAYS HOME</span>
            <h2>只走本地 · 数据不出本机</h2>
            <p className="sub">
              打开后,路由<strong>只用你标为「本地」的供应商</strong>,请求绝不出本机。
              {hasLocalProvider
                ? "改完按上方「保存并应用」生效。"
                : "还没有本地供应商——去「添加供应商」时勾选「本地模型」。"}
            </p>
          </div>
          <span className="default-route-chip">隐私优先</span>
        </div>

        <label className="switch-row">
          <input
            type="checkbox"
            checked={localOnly}
            disabled={busy || !hasLocalProvider}
            onChange={(event) =>
              onSetLocalRouting(event.target.checked, event.target.checked && allowCloudFallback)
            }
          />
          <span>只走本地模型(请求不出本机)</span>
        </label>
        {localOnly && (
          <label className="switch-row switch-row-sub">
            <input
              type="checkbox"
              checked={allowCloudFallback}
              disabled={busy}
              onChange={(event) => onSetLocalRouting(true, event.target.checked)}
            />
            <span>
              本地不可用时,允许退到云模型兜底
              <em>(关=严格本地,本地挂了宁可失败也不外发)</em>
            </span>
          </label>
        )}
      </section>

      <RecentReceipts />

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
