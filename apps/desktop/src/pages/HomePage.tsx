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
import QuotaPriorityPanel from "../components/QuotaPriorityPanel";
import { useLocalizedCopy } from "../components/LanguageProvider";
import RoutingModeSelector from "../components/RoutingModeSelector";
import DirectRoutePanel from "../components/DirectRoutePanel";
import { Button } from "../components/ui/button";
import { Input } from "../components/ui/input";

interface HomePageProps {
  providers: ProviderView[];
  tiers: Record<TierSlot, TierView>;
  keywords: Record<TierSlot, string[]>;
  profiles: string[];
  routingMode: RoutingMode;
  directTarget?: DirectRouteTarget | null;
  onSetRoutingMode: (mode: RoutingMode) => void;
  onApplyDirect: (upstream: string, model: string) => boolean | void | Promise<boolean | void>;
  onDirectDraftChange?: (hasUnappliedTarget: boolean) => void;
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
  onTierChange: (slot: TierSlot, upstream: string | null, model: string | null) => void;
  onSaveProfile: (name: string) => Promise<boolean>;
  onDeleteProfile: (name: string) => Promise<boolean>;
  onAddKeyword: (slot: TierSlot, keyword: string) => boolean | void | Promise<boolean | void>;
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
  onDirectDraftChange,
  quotaAccounts,
  onSaveQuota,
  onSaveQuotaPlan,
  onViewQuotaUsage,
  busy,
  applying,
  configError,
  saveStatus,
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
          onDraftChange={onDirectDraftChange}
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
            <Button
              variant="outline"
              type="button"
              aria-expanded={profileOpen}
              disabled={busy || profileBusy}
              onClick={() => setProfileOpen((current) => !current)}
            >
              {profileOpen ? copy("Close", "收起", "關閉", "閉じる") : copy("Save as profile", "存为策略", "存為策略群組", "プロファイルとして保存")}
            </Button>
            <Button variant="outline" type="button" disabled={busy || applying} onClick={onApplyAll}>
              {copy("Apply to all Agents", "应用到全部 Agent", "應用到全部 Agent", "すべての Agent に適用")}
            </Button>
            <Button
              type="button"
              data-onboarding-target="route-apply"
              disabled={busy || applying}
              onClick={onSave}
            >
              {applying ? copy("Applying…", "应用中…", "應用中…", "適用中…") : copy("Save and apply", "保存并应用", "儲存並應用", "保存して適用")}
            </Button>
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
            keywords={keywords}
            onAddKeyword={onAddKeyword}
            onRemoveKeyword={onRemoveKeyword}
            onTierChange={onTierChange}
          />
        </div>

        {profileOpen && <div className="profile-manager compact-profile-manager">
          <div className="profile-create-row">
            <Input
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
            <Button type="button" disabled={busy || profileBusy || !profileName.trim()} onClick={() => void saveProfile()}>
              {copy("Save profile", "保存策略", "儲存策略群組", "プロファイルを保存")}
            </Button>
            <Button variant="outline" type="button" disabled={profileBusy} onClick={() => setProfileOpen(false)}>
              {copy("Cancel", "取消", "取消", "キャンセル")}
            </Button>
          </div>
        </div>}

        {profiles.length > 0 && (
          <div className="profile-list profile-list-visible" aria-label={copy("Saved profiles", "已有策略组", "已有策略群組", "保存済みプロファイル")}>
            {profiles.map((profile) => (
              <span key={profile}>
                {profile}
                <Button
                  variant="ghost"
                  size="icon-xs"
                  type="button"
                  aria-label={copy(`Delete profile ${profile}`, `删除策略组 ${profile}`, `刪除策略組 ${profile}`, `プロファイル ${profile} を削除`)}
                  disabled={busy || profileBusy}
                  onClick={() => void removeProfile(profile)}
                >
                  ×
                </Button>
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

      </>
      )}

    </div>
  );
}
