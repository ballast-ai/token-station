import { useState } from "react";
import {
  type ProviderView,
  type QuotaAccount,
  type StateView,
  type TierSlot,
  type TierView,
} from "../api";
import TierRouteEditor from "../components/TierRouteEditor";
import TierKeywords from "../components/TierKeywords";
import ProviderList from "../components/ProviderList";
import RecentReceipts from "../components/RecentReceipts";
import QuotaPriorityPanel from "../components/QuotaPriorityPanel";
import { useLocalizedCopy } from "../components/LanguageProvider";

interface HomePageProps {
  providers: ProviderView[];
  deletedProviders: string[];
  providerRecoveryError: string | null;
  tiers: Record<TierSlot, TierView>;
  keywords: Record<TierSlot, string[]>;
  profiles: string[];
  routingMode: "tiered" | "quota_first";
  quotaAccounts: QuotaAccount[];
  onSaveQuota: (accounts: QuotaAccount[]) => void;
  serveRunning: boolean;
  busy: boolean;
  applying: boolean;
  configError: string | null;
  saveStatus: string;
  localOnly: boolean;
  allowCloudFallback: boolean;
  onSetLocalRouting: (localOnly: boolean, allowCloudFallback: boolean) => void;
  onTierChange: (slot: TierSlot, upstream: string | null, model: string | null) => void;
  onSaveProfile: (name: string) => Promise<boolean>;
  onDeleteProfile: (name: string) => Promise<boolean>;
  onAddKeyword: (slot: TierSlot, keyword: string) => void;
  onRemoveKeyword: (slot: TierSlot, keyword: string) => void;
  onSave: () => void;
  onApplyAll: () => void;
  onRemoveProvider: (name: string) => void;
  onRestoreProvider: (name: string) => void;
  onStateChange: (state: StateView, message: string) => void;
}

export default function HomePage({
  providers,
  deletedProviders,
  providerRecoveryError,
  tiers,
  keywords,
  profiles,
  routingMode,
  quotaAccounts,
  onSaveQuota,
  serveRunning,
  busy,
  applying,
  configError,
  saveStatus,
  localOnly,
  allowCloudFallback,
  onSetLocalRouting,
  onTierChange,
  onSaveProfile,
  onDeleteProfile,
  onAddKeyword,
  onRemoveKeyword,
  onSave,
  onApplyAll,
  onRemoveProvider,
  onRestoreProvider,
  onStateChange,
}: HomePageProps) {
  const { copy } = useLocalizedCopy();
  const tierConfigured: Record<TierSlot, boolean> = {
    high: Boolean(tiers.high?.upstream && tiers.high?.model),
    mid: Boolean(tiers.mid?.upstream && tiers.mid?.model),
    low: Boolean(tiers.low?.upstream && tiers.low?.model),
  };
  const hasLocalProvider = providers.some((provider) => provider.local);
  const [profileName, setProfileName] = useState("");
  const [profileBusy, setProfileBusy] = useState(false);
  const [profileOpen, setProfileOpen] = useState(false);

  const saveProfile = async () => {
    const name = profileName.trim();
    if (!name || profileBusy) return;
    setProfileBusy(true);
    try {
      if (await onSaveProfile(name)) {
        setProfileName("");
        setProfileOpen(false);
      }
    } finally {
      setProfileBusy(false);
    }
  };

  const removeProfile = async (name: string) => {
    if (profileBusy) return;
    setProfileBusy(true);
    try {
      await onDeleteProfile(name);
    } finally {
      setProfileBusy(false);
    }
  };
  return (
    <div className="page-stack home-page">
      <header className="page-title-row">
        <div>
          <h1>{copy("Home routing", "主页路由")}</h1>
          <p>{copy(
            "These three tiers are the default for every Agent. Individual routes only override their Agent.",
            "这套三档配置是所有 Agent 的默认值。独立路由只覆盖对应 Agent。",
          )}</p>
        </div>
      </header>

      {routingMode === "quota_first" ? (
        <QuotaPriorityPanel
          providers={providers}
          accounts={quotaAccounts}
          busy={busy}
          applying={applying}
          onSave={onSaveQuota}
        />
      ) : (
      <>
      <section className="panel route-panel">
        <div className="panel-head split-heading">
          <div>
            <h2>{copy("Smart routing", "智能路由")}</h2>
            <p className="sub">{copy(
              "Choose models by task complexity.",
              "根据任务复杂度选择不同模型。",
            )}</p>
          </div>
          <div className="route-heading-actions">
            {profiles.length > 0 && (
              <span className="count-badge">
                {copy(
                  `${profiles.length} ${profiles.length === 1 ? "profile" : "profiles"}`,
                  `${profiles.length} 个策略`,
                )}
              </span>
            )}
            <button
              className="btn quiet"
              type="button"
              aria-expanded={profileOpen}
              disabled={busy || profileBusy}
              onClick={() => setProfileOpen((current) => !current)}
            >
              {profileOpen ? copy("Close", "收起") : copy("Save as profile", "存为策略")}
            </button>
          </div>
        </div>

        <TierRouteEditor
          tiers={tiers}
          providers={providers}
          disabled={busy}
          onTierChange={onTierChange}
        />

        {profileOpen && <div className="profile-manager compact-profile-manager">
          <div className="profile-create-row">
            <input
              className="input"
              aria-label={copy("Profile name", "策略组名称")}
              value={profileName}
              maxLength={80}
              placeholder={copy("For example: Daily development", "例如：日常开发")}
              disabled={busy || profileBusy}
              onChange={(event) => setProfileName(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter") void saveProfile();
              }}
            />
            <button className="btn primary" type="button" disabled={busy || profileBusy || !profileName.trim()} onClick={() => void saveProfile()}>
              {copy("Save profile", "保存策略")}
            </button>
            <button className="btn quiet" type="button" disabled={profileBusy} onClick={() => setProfileOpen(false)}>
              {copy("Cancel", "取消")}
            </button>
          </div>
          {profiles.length > 0 && (
            <div className="profile-list" aria-label={copy("Saved profiles", "已有策略组")}>
              {profiles.map((profile) => (
                <span key={profile}>
                  {profile}
                  <button
                    type="button"
                    aria-label={copy(`Delete profile ${profile}`, `删除策略组 ${profile}`)}
                    disabled={busy || profileBusy}
                    onClick={() => void removeProfile(profile)}
                  >
                    ×
                  </button>
                </span>
              ))}
            </div>
          )}
        </div>}

        <footer className="panel-foot route-actions">
          <div className="route-status-copy">
            <span className="foot-hint" data-testid="config-save-status">{saveStatus}</span>
            {providers.length === 0 && (
              <span className="foot-hint">
                {copy("Add a provider before configuring the tiers.", "请先添加供应商，再配置三档。")}
              </span>
            )}
            {providers.length > 0 && configError && (
              <span className="foot-hint">
                {copy(
                  "Some tiers are incomplete. Saving will run full validation.",
                  "还有档位未完成，保存时会进行完整校验。",
                )}
              </span>
            )}
          </div>
          <div className="route-action-buttons">
            <button className="btn" type="button" disabled={busy || applying} onClick={onApplyAll}>
              {copy("Apply to all Agents", "应用到全部 Agent")}
            </button>
            <button className="btn primary" type="button" disabled={busy || applying} onClick={onSave}>
              {applying ? copy("Applying…", "应用中…") : copy("Save and apply", "保存并应用")}
            </button>
          </div>
        </footer>
      </section>

      <section className="panel keyword-panel">
        <div className="panel-head split-heading">
          <div>
            <span className="eyebrow">KEYWORD OVERRIDE · YOU'RE IN CONTROL</span>
            <h2>{copy("Keyword routing", "关键词路由 · 你说了算")}</h2>
            <p className="sub">
              {copy(
                "Add a keyword to a tier to override automatic classification whenever a request contains it. Save and apply when finished.",
                "自动分档不称心？给某一档加个关键词，以后请求里只要出现它，就固定到这一档，优先于自动判断。加完按上方“保存并应用”生效。",
              )}
            </p>
          </div>
          <span className="default-route-chip">{copy("Highest priority", "最高优先级")}</span>
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
            <h2>{copy("Local only", "只走本地 · 数据不出本机")}</h2>
            <p className="sub">
              {hasLocalProvider
                ? copy(
                    "Only providers marked as local will be used, so requests stay on this Mac. Save and apply when finished.",
                    "打开后，路由只使用标为“本地”的供应商，请求不会离开本机。改完按上方“保存并应用”生效。",
                  )
                : copy(
                    "No local provider is configured. Add a provider and mark it as a local model first.",
                    "还没有本地供应商。请先添加供应商并勾选“本地模型”。",
                  )}
            </p>
          </div>
          <span className="default-route-chip">{copy("Privacy first", "隐私优先")}</span>
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
          <span>{copy("Use local models only", "只走本地模型（请求不出本机）")}</span>
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
              {copy(
                "Allow cloud fallback when local models are unavailable",
                "本地不可用时，允许使用云模型兜底",
              )}
              <em>{copy(
                "(Off means strict local mode; requests fail instead of leaving this Mac.)",
                "（关闭后为严格本地模式，本地不可用时请求会失败，不会外发。）",
              )}</em>
            </span>
          </label>
        )}
      </section>
      </>
      )}

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
