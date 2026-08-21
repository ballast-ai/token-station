import { useState } from "react";
import { previewProviderRemoval } from "../api";
import type { ProviderRemovalPreview, ProviderView, StateView } from "../api";
import ProviderModelManager from "./ProviderModelManager";
import { useLocalizedCopy } from "./LanguageProvider";
import { humanizeAppError } from "../errors";
import { ProviderIcon } from "../brandIcons";
import { useErrorToast } from "./ErrorToast";

interface ProviderListProps {
  providers: ProviderView[];
  deletedProviders: string[];
  recoveryError: string | null;
  serveRunning: boolean;
  busy: boolean;
  onRemove: (name: string) => void;
  onRestore: (name: string) => void;
  onStateChange: (state: StateView) => void;
}

export default function ProviderList({
  providers,
  deletedProviders,
  recoveryError,
  serveRunning,
  busy,
  onRemove,
  onRestore,
  onStateChange,
}: ProviderListProps) {
  const { copy, language } = useLocalizedCopy();
  const { showError } = useErrorToast();
  const [managedProvider, setManagedProvider] = useState<string | null>(null);
  const [removal, setRemoval] = useState<ProviderRemovalPreview | null>(null);
  const modelCount = providers.reduce((total, provider) => total + provider.models.length, 0);

  const inspectRemoval = async (name: string) => {
    try {
      setRemoval(await previewProviderRemoval(name));
    } catch (caught) {
      showError(humanizeAppError(caught, language), `provider-removal-preview:${name}`);
    }
  };

  return (
    <section className="panel provider-panel">
      <div className="panel-head split-heading">
        <div>
          <h2>{copy("Managed models", "已接入模型", "已接入模型", "接続済みモデル")}</h2>
        </div>
        <span className="count-badge">
          {copy(`${modelCount} models`, `${modelCount} 个模型`, `${modelCount} 個模型`, `${modelCount} 個モデル`)}
        </span>
      </div>

      <div className="provider-list">
        {recoveryError && (
          <div className="manager-error">{humanizeAppError(recoveryError, language)}</div>
        )}
        {deletedProviders.length > 0 && (
          <div className="provider-recovery" aria-label={copy("Provider recycle bin", "Provider 回收站", "供應商回收站", "プロバイダーごみ箱")}>
            <strong>{copy("Recoverable providers", "可恢复的 Provider", "可恢復的供應商", "復元可能なプロバイダー")}</strong>
            {deletedProviders.map((name) => (
              <button className="btn tiny" type="button" disabled={busy} key={name} onClick={() => onRestore(name)}>
                {copy(`Restore ${name}`, `恢复 ${name}`, `恢復 ${name}`, `${name} を復元`)}
              </button>
            ))}
          </div>
        )}
        {providers.length === 0 && (
          <div className="empty-state">
            <strong>{copy("No models yet", "还没有模型", "還沒有模型", "まだモデルがありません")}</strong>
            <span>{copy(
              "Select Add model in the top-right corner to get started.",
              "点击右上角“添加模型”开始配置。", "點選右上角「新增模型」開始配置。", "右上角の「モデルを追加」をクリックして設定を開始してください。"
            )}</span>
          </div>
        )}
        {providers.map((provider) => (
          <article
            className={`provider-card ${managedProvider === provider.name ? "expanded" : ""}`}
            key={provider.name}
            role="group"
            aria-label={copy(`${provider.name} provider`, `${provider.name} 供应商`, `${provider.name} 供應商`, `${provider.name} プロバイダー`)}
          >
            <div className="provider-card-head provider-identity-bar">
              <div className="provider-monogram" aria-hidden="true">
                <ProviderIcon id={provider.brand_id} label={provider.name} size={34} />
              </div>
              <div className="provider-main provider-identity">
                <small>{copy("Provider", "供应商", "供應商", "プロバイダー")}</small>
                <strong className="provider-identity-name">{provider.name}</strong>
                <div className="provider-url">{provider.base_url}</div>
              </div>
              <span className="provider-model-count">
                {copy(`${provider.models.length} models`, `${provider.models.length} 个模型`, `${provider.models.length} 個模型`, `${provider.models.length} 個モデル`)}
              </span>
              <div className="provider-side">
                {provider.access_tier === "free" && (
                  <span className="provider-access-badge">{copy("Free", "免费", "免費", "無料")}</span>
                )}
                <span className={`auth ${provider.has_auth ? "yes" : "no"}`}>
                  {provider.has_auth
                    ? copy(
                      `Credential: ${provider.credential_source ?? "store"}`,
                      `凭据：${provider.credential_source ?? "store"}`, `憑據：${provider.credential_source ?? "store"}`, `資格：${provider.credential_source ?? "store"}`
                    )
                    : copy("No authentication", "无鉴权", "無認證", "認証なし")}
                </span>
                <button
                  className="btn tiny"
                  type="button"
                  disabled={busy}
                  onClick={() => setManagedProvider((current) => current === provider.name ? null : provider.name)}
                >
                  {managedProvider === provider.name
                    ? copy("Close", "收起", "關閉", "閉じる")
                    : copy("Manage", "管理详情", "管理", "管理")}
                </button>
                <button className="btn tiny danger" type="button" disabled={busy} onClick={() => void inspectRemoval(provider.name)}>
                  {copy("Delete", "删除", "刪除", "削除")}
                </button>
              </div>
            </div>
            <div
              className="provider-primary-models"
              role="list"
              aria-label={copy(`${provider.name} models`, `${provider.name} 模型`, `${provider.name} 模型`, `${provider.name} モデル`)}
              data-layout="compact-three-column"
            >
              {provider.models.length > 0 ? provider.models.map((model) => (
                <div role="listitem" key={model} title={`${model} · ${provider.name}`}>
                  <strong>{model}</strong>
                  <small>{copy("Provider", "供应商", "供應商", "プロバイダー")} · {provider.name}</small>
                </div>
              )) : (
                <div role="listitem">
                  <strong>{copy("No managed models", "暂无已管理模型", "目前無已管理模型", "現在管理されていないモデルがあります")}</strong>
                  <small>{copy("Provider", "供应商", "供應商", "プロバイダー")} · {provider.name}</small>
                </div>
              )}
            </div>
            {managedProvider === provider.name && (
              provider.access_tier === "free" ? (
                <div className="free-provider-managed-note">
                  {copy(
                    "Free provider endpoints and model sets are protected by the catalog. To change the key or models, delete this provider and add it again through the free catalog.",
                    "免费实例的端点与模型集合受免费目录保护。需要更换 Key 或模型时，请删除后从免费目录重新验证添加。", "免費例項的端點與模型集合受免費目錄保護。需要更換 Key 或模型時，請刪除後從免費目錄重新驗證新增。", "無料インスタンスのエンドポイントとモデルセットは無料カタログによって保護されています。Keyやモデルを変更する場合は、このプロバイダーを削除し、無料カタログから再度追加してください。"
                  )}
                </div>
              ) : (
                <ProviderModelManager
                  provider={provider}
                  serveRunning={serveRunning}
                  disabled={busy}
                  onSaved={onStateChange}
                />
              )
            )}
            {removal?.name === provider.name && (
              <div
                className="provider-removal-preview"
                role="dialog"
                aria-label={copy("Deletion impact preview", "删除影响预览", "刪除影響預覽", "削除影響プレビュー")}
              >
                <strong>{copy("Deletion impact", "删除影响", "刪除影響", "削除影響")}</strong>
                {removal.references.length > 0 ? (
                  <>
                    <p>{copy(
                      "The following routes still reference this provider and must be updated first:",
                      "以下路由仍在引用，必须先调整：", "以下路由仍在引用，必須先調整：", "以下のルートがまだ参照しています。まず調整してください："
                    )}</p>
                    <ul>{removal.references.map((reference) => <li key={reference}>{reference}</li>)}</ul>
                  </>
                ) : (
                  <p>{copy(
                    "No routes reference this provider. Deleted providers move to the local recycle bin and can be restored.",
                    "没有路由引用。删除后会进入本地回收站，可恢复。", "沒有路由引用。刪除後會進入本地回收站，可恢復。", "ルートが参照していません。削除後はローカルのゴミ箱に移動し、復元可能です。"
                  )}</p>
                )}
                <div>
                  <button className="btn tiny" type="button" onClick={() => setRemoval(null)}>
                    {copy("Cancel", "取消", "取消", "キャンセル")}
                  </button>
                  <button
                    className="btn tiny danger"
                    type="button"
                    disabled={busy || !removal.can_remove}
                    onClick={() => onRemove(provider.name)}
                  >
                    {copy("Move to recycle bin", "确认移入回收站", "確認移入回收站", "ゴミ箱に移動")}
                  </button>
                </div>
              </div>
            )}
          </article>
        ))}
      </div>
    </section>
  );
}
