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
          <h2>{copy("Managed models", "已接入模型")}</h2>
        </div>
        <span className="count-badge">
          {copy(`${modelCount} models`, `${modelCount} 个模型`)}
        </span>
      </div>

      <div className="provider-list">
        {recoveryError && (
          <div className="manager-error">{humanizeAppError(recoveryError, language)}</div>
        )}
        {deletedProviders.length > 0 && (
          <div className="provider-recovery" aria-label={copy("Provider recycle bin", "Provider 回收站")}>
            <strong>{copy("Recoverable providers", "可恢复的 Provider")}</strong>
            {deletedProviders.map((name) => (
              <button className="btn tiny" type="button" disabled={busy} key={name} onClick={() => onRestore(name)}>
                {copy(`Restore ${name}`, `恢复 ${name}`)}
              </button>
            ))}
          </div>
        )}
        {providers.length === 0 && (
          <div className="empty-state">
            <strong>{copy("No models yet", "还没有模型")}</strong>
            <span>{copy(
              "Select Add model in the top-right corner to get started.",
              "点击右上角“添加模型”开始配置。",
            )}</span>
          </div>
        )}
        {providers.map((provider) => (
          <article
            className={`provider-card ${managedProvider === provider.name ? "expanded" : ""}`}
            key={provider.name}
            role="group"
            aria-label={copy(`${provider.name} provider`, `${provider.name} 供应商`)}
          >
            <div className="provider-card-head provider-identity-bar">
              <div className="provider-monogram" aria-hidden="true">
                <ProviderIcon id={provider.brand_id} label={provider.name} size={34} />
              </div>
              <div className="provider-main provider-identity">
                <small>{copy("Provider", "供应商")}</small>
                <strong className="provider-identity-name">{provider.name}</strong>
                <div className="provider-url">{provider.base_url}</div>
              </div>
              <span className="provider-model-count">
                {copy(`${provider.models.length} models`, `${provider.models.length} 个模型`)}
              </span>
              <div className="provider-side">
                {provider.access_tier === "free" && (
                  <span className="provider-access-badge">{copy("Free", "免费")}</span>
                )}
                <span className={`auth ${provider.has_auth ? "yes" : "no"}`}>
                  {provider.has_auth
                    ? copy(
                      `Credential: ${provider.credential_source ?? "store"}`,
                      `凭据：${provider.credential_source ?? "store"}`,
                    )
                    : copy("No authentication", "无鉴权")}
                </span>
                <button
                  className="btn tiny"
                  type="button"
                  disabled={busy}
                  onClick={() => setManagedProvider((current) => current === provider.name ? null : provider.name)}
                >
                  {managedProvider === provider.name
                    ? copy("Close", "收起")
                    : copy("Manage", "管理详情")}
                </button>
                <button className="btn tiny danger" type="button" disabled={busy} onClick={() => void inspectRemoval(provider.name)}>
                  {copy("Delete", "删除")}
                </button>
              </div>
            </div>
            <div
              className="provider-primary-models"
              role="list"
              aria-label={copy(`${provider.name} models`, `${provider.name} 模型`)}
              data-layout="compact-three-column"
            >
              {provider.models.length > 0 ? provider.models.map((model) => (
                <div role="listitem" key={model} title={`${model} · ${provider.name}`}>
                  <strong>{model}</strong>
                  <small>{copy("Provider", "供应商")} · {provider.name}</small>
                </div>
              )) : (
                <div role="listitem">
                  <strong>{copy("No managed models", "暂无已管理模型")}</strong>
                  <small>{copy("Provider", "供应商")} · {provider.name}</small>
                </div>
              )}
            </div>
            {managedProvider === provider.name && (
              provider.access_tier === "free" ? (
                <div className="free-provider-managed-note">
                  {copy(
                    "Free provider endpoints and model sets are protected by the catalog. To change the key or models, delete this provider and add it again through the free catalog.",
                    "免费实例的端点与模型集合受免费目录保护。需要更换 Key 或模型时，请删除后从免费目录重新验证添加。",
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
                aria-label={copy("Deletion impact preview", "删除影响预览")}
              >
                <strong>{copy("Deletion impact", "删除影响")}</strong>
                {removal.references.length > 0 ? (
                  <>
                    <p>{copy(
                      "The following routes still reference this provider and must be updated first:",
                      "以下路由仍在引用，必须先调整：",
                    )}</p>
                    <ul>{removal.references.map((reference) => <li key={reference}>{reference}</li>)}</ul>
                  </>
                ) : (
                  <p>{copy(
                    "No routes reference this provider. Deleted providers move to the local recycle bin and can be restored.",
                    "没有路由引用。删除后会进入本地回收站，可恢复。",
                  )}</p>
                )}
                <div>
                  <button className="btn tiny" type="button" onClick={() => setRemoval(null)}>
                    {copy("Cancel", "取消")}
                  </button>
                  <button
                    className="btn tiny danger"
                    type="button"
                    disabled={busy || !removal.can_remove}
                    onClick={() => onRemove(provider.name)}
                  >
                    {copy("Move to recycle bin", "确认移入回收站")}
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
