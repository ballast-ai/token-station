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
          <h2>{copy("Providers", "供应商")}</h2>
          <p className="sub">{copy(
            "Manage providers and available models in one catalog shared by Home and every Agent.",
            "统一维护供应商和可用模型，主页与所有客户端共用这一份目录。",
          )}</p>
        </div>
        <span className="count-badge">
          {copy(`${providers.length} total`, `${providers.length} 个`)}
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
            <strong>{copy("No providers yet", "还没有供应商")}</strong>
            <span>{copy(
              "Select Add provider in the top-right corner to get started.",
              "点击右上角“添加供应商”开始配置。",
            )}</span>
          </div>
        )}
        {providers.map((provider) => (
          <article className={`provider-card ${managedProvider === provider.name ? "expanded" : ""}`} key={provider.name}>
            <div className="provider-card-head">
              <div className="provider-monogram" aria-hidden="true">
                <ProviderIcon id={provider.brand_id} label={provider.name} size={34} />
              </div>
              <div className="provider-main">
                <div className="provider-name">
                  {provider.name}
                  {provider.access_tier === "free" && (
                    <span className="provider-access-badge">{copy("Free", "免费")}</span>
                  )}
                </div>
                <div className="provider-url">{provider.base_url}</div>
                <div className="provider-models">
                  {provider.models.slice(0, 4).map((model) => <span className="chip" key={model}>{model}</span>)}
                  {provider.models.length > 4 && <span className="chip quiet-chip">+{provider.models.length - 4}</span>}
                </div>
              </div>
              <div className="provider-side">
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
