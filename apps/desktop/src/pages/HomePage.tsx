import { useState } from "react";
import {
  type DirectRouteTarget,
  type ProviderView,
  type QuotaAccount,
  type RoutingMode,
  type TierSlot,
  type TierView,
} from "../api";
import TierRouteEditor from "../components/TierRouteEditor";
import TierKeywords from "../components/TierKeywords";
import QuotaPriorityPanel from "../components/QuotaPriorityPanel";
import { useLocalizedCopy } from "../components/LanguageProvider";
import RoutingModeSelector from "../components/RoutingModeSelector";
import DirectRoutePanel from "../components/DirectRoutePanel";

interface HomePageProps {
  providers: ProviderView[];
  tiers: Record<TierSlot, TierView>;
  keywords: Record<TierSlot, string[]>;
  profiles: string[];
  routingMode: RoutingMode;
  directTarget?: DirectRouteTarget | null;
  onSetRoutingMode: (mode: RoutingMode) => void;
  onApplyDirect: (upstream: string, model: string) => void;
  quotaAccounts: QuotaAccount[];
  onSaveQuota: (accounts: QuotaAccount[]) => void;
  onSaveQuotaPlan: (
    upstream: string,
    lenMs: number,
    limit: number,
    unit: "tokens" | "requests",
  ) => void;
  onViewQuotaUsage: () => void;
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
  embedded?: boolean;
}

export default function HomePage({
  providers,
  tiers,
  keywords,
  profiles,
  routingMode,
  directTarget,
  onSetRoutingMode,
  onApplyDirect,
  quotaAccounts,
  onSaveQuota,
  onSaveQuotaPlan,
  onViewQuotaUsage,
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
  embedded = false,
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
  const [keywordTier, setKeywordTier] = useState<TierSlot | null>(null);

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
      {!embedded && (
        <header className="page-title-row">
          <div>
            <h1>{copy("Global routing", "全局路由", "全域路由", "グローバルルーティング")}</h1>
          </div>
        </header>
      )}

      <RoutingModeSelector
        value={routingMode}
        disabled={busy}
        onValueChange={onSetRoutingMode}
      />

      {routingMode === "direct" ? (
        <DirectRoutePanel
          providers={providers}
          target={directTarget}
          busy={busy}
          applying={applying}
          onApply={onApplyDirect}
        />
      ) : routingMode === "quota_first" ? (
        <QuotaPriorityPanel
          providers={providers}
          accounts={quotaAccounts}
          busy={busy}
          applying={applying}
          onSave={onSaveQuota}
          onViewUsage={onViewQuotaUsage}
          onSavePlan={onSaveQuotaPlan}
        />
      ) : (
      <>
      <section className="panel route-panel">
        <div className="panel-head split-heading">
          <div>
            <h2>{copy("Smart routing", "智能路由", "智慧路由", "スマートルーティング")}</h2>
            <p className="sub">{copy(
              "Choose models by task complexity.",
              "根据任务复杂度选择不同模型。", "根據任務複雜度選擇不同模型。", "タスクの複雑さに応じてモデルを選択します。"
            )}</p>
          </div>
          <div className="route-heading-actions">
            {profiles.length > 0 && (
              <span className="count-badge">
                {copy(
                  `${profiles.length} ${profiles.length === 1 ? "profile" : "profiles"}`,
                  `${profiles.length} 个策略`,
                  `${profiles.length} 個策略群組`,
                  `${profiles.length} プロファイル`,
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
              {profileOpen ? copy("Close", "收起", "關閉", "閉じる") : copy("Save as profile", "存为策略", "存為策略群組", "プロファイルとして保存")}
            </button>
            <button className="btn" type="button" disabled={busy || applying} onClick={onApplyAll}>
              {copy("Apply to all Agents", "应用到全部 Agent", "應用到全部 Agent", "すべての Agent に適用")}
            </button>
            <button
              className="btn primary"
              type="button"
              data-onboarding-target="route-apply"
              disabled={busy || applying}
              onClick={onSave}
            >
              {applying ? copy("Applying…", "应用中…", "應用中…", "適用中…") : copy("Save and apply", "保存并应用", "儲存並應用", "保存して適用")}
            </button>
          </div>
        </div>

        <div
          role="group"
          aria-label={copy("Three-tier model configuration", "三档模型配置", "三檔模型配置", "三段階モデル設定")}
          data-onboarding-target="route-config"
        >
          <TierRouteEditor
            tiers={tiers}
            providers={providers}
            disabled={busy}
            keywordCounts={{
              high: keywords.high.length,
              mid: keywords.mid.length,
              low: keywords.low.length,
            }}
            onEditKeywords={setKeywordTier}
            onTierChange={onTierChange}
          />
        </div>

        {profileOpen && <div className="profile-manager compact-profile-manager">
          <div className="profile-create-row">
            <input
              className="input"
              aria-label={copy("Profile name", "策略组名称", "策略群組名稱", "プロファイル名")}
              value={profileName}
              maxLength={80}
              placeholder={copy("For example: Daily development", "例如：日常开发", "例如：日常開發", "例：日常開発")}
              disabled={busy || profileBusy}
              onChange={(event) => setProfileName(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter") void saveProfile();
              }}
            />
            <button className="btn primary" type="button" disabled={busy || profileBusy || !profileName.trim()} onClick={() => void saveProfile()}>
              {copy("Save profile", "保存策略", "儲存策略群組", "プロファイルを保存")}
            </button>
            <button className="btn quiet" type="button" disabled={profileBusy} onClick={() => setProfileOpen(false)}>
              {copy("Cancel", "取消", "取消", "キャンセル")}
            </button>
          </div>
        </div>}

        {profiles.length > 0 && (
          <div className="profile-list profile-list-visible" aria-label={copy("Saved profiles", "已有策略组", "已有策略群組", "保存済みプロファイル")}>
            {profiles.map((profile) => (
              <span key={profile}>
                {profile}
                <button
                  type="button"
                  aria-label={copy(`Delete profile ${profile}`, `删除策略组 ${profile}`, `刪除策略組 ${profile}`, `プロファイル ${profile} を削除`)}
                  disabled={busy || profileBusy}
                  onClick={() => void removeProfile(profile)}
                >
                  ×
                </button>
              </span>
            ))}
          </div>
        )}

        <footer className="panel-foot route-actions">
          <div className="route-status-copy">
            <span className="foot-hint" data-testid="config-save-status">{saveStatus}</span>
            {providers.length === 0 && (
              <span className="foot-hint">
                {copy("Add a provider before configuring the tiers.", "请先添加供应商，再配置三档。", "請先新增供應商，再配置三檔。", "まずプロバイダーを追加し、その後三段階設定を行ってください。")}
              </span>
            )}
            {providers.length > 0 && configError && (
              <span className="foot-hint">
                {copy(
                  "Some tiers are incomplete. Saving will run full validation.",
                  "还有档位未完成，保存时会进行完整校验。", "還有檔位未完成，儲存時會進行完整校驗。", "まだ段階が未完成です。保存時に完全な検証が行われます。"
                )}
              </span>
            )}
          </div>
        </footer>
      </section>

      <TierKeywords
        keywords={keywords}
        configured={tierConfigured}
        activeSlot={keywordTier}
        disabled={busy}
        onOpenChange={(open) => !open && setKeywordTier(null)}
        onAdd={onAddKeyword}
        onRemove={onRemoveKeyword}
      />

      <section className="panel local-routing-panel">
        <div className="panel-head split-heading">
          <div>
            <h2>{copy("Local only", "只走本地", "只走本地", "ローカルのみ")}</h2>
            <p className="sub">
              {hasLocalProvider
                ? copy(
                    "Only providers marked as local will be used, so requests stay on this Mac. Save and apply when finished.",
                    "打开后，路由只使用标为“本地”的供应商，请求不会离开本机。改完按上方“保存并应用”生效。", "開啟後，路由只使用標為「本地」的供應商，請求不會離開本機。改完按上方「儲存並應用」生效。", "開くと、ルーティングは「ローカル」にマークされたプロバイダーのみを使用し、リクエストがこのマシンの外に漏れません。変更が完了したら上記の「保存して適用」をクリックしてください。"
                  )
                : copy(
                    "No local provider is configured. Add a provider and mark it as a local model first.",
                    "还没有本地供应商。请先添加供应商并勾选“本地模型”。", "尚未設定任何本機供應商。請先新增供應商並勾選「本機模型」。", "ローカルプロバイダーが設定されていません。まずプロバイダーを追加し、「ローカルモデル」にチェックしてください。"
                  )}
            </p>
          </div>
          <span className="default-route-chip">{copy("Privacy first", "隐私优先", "隱私優先", "プライバシー優先")}</span>
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
          <span>{copy("Use local models only", "只走本地模型（请求不出本机）", "僅使用本機模型（請求不會離開本機）", "ローカルモデルのみを使用（リクエストはローカルにのみ発生）")}</span>
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
                "本地不可用时，允许使用云模型兜底", "本地不可用時，允許使用雲模型作為備用", "ローカルモデルが利用不可な場合、クラウドモデルをバックアップとして使用を許可"
              )}
              <em>{copy(
                "(Off means strict local mode; requests fail instead of leaving this Mac.)",
                "（关闭后为严格本地模式，本地不可用时请求会失败，不会外发。）", "（關閉後為嚴格本地模式，本地不可用時請求會失敗，不會外發。）", "（オフにすると厳格なローカルモードになります。ローカルモデルが利用不可な場合、リクエストは失敗し、外部に送信されません。）"
              )}</em>
            </span>
          </label>
        )}
      </section>
      </>
      )}

    </div>
  );
}
