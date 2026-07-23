import { useState } from "react";
import { previewProviderRemoval } from "../api";
import type { ProviderRemovalPreview, ProviderView, StateView } from "../api";
import ProviderModelManager from "./ProviderModelManager";

interface ProviderListProps {
  providers: ProviderView[];
  deletedProviders: string[];
  recoveryError: string | null;
  serveRunning: boolean;
  busy: boolean;
  onRemove: (name: string) => void;
  onRestore: (name: string) => void;
  onStateChange: (state: StateView, message: string) => void;
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
  const [managedProvider, setManagedProvider] = useState<string | null>(null);
  const [removal, setRemoval] = useState<ProviderRemovalPreview | null>(null);
  const [removalError, setRemovalError] = useState("");

  const inspectRemoval = async (name: string) => {
    setRemovalError("");
    try {
      setRemoval(await previewProviderRemoval(name));
    } catch (caught) {
      setRemovalError(String(caught));
    }
  };

  return (
    <section className="panel provider-panel">
      <div className="panel-head split-heading">
        <div>
          <span className="eyebrow">UPSTREAMS</span>
          <h2>供应商</h2>
          <p className="sub">统一维护供应商和可用模型，主页与五个 Agent 共用这一份目录。</p>
        </div>
        <span className="count-badge">{providers.length} 个</span>
      </div>

      <div className="provider-list">
        {recoveryError && <div className="manager-error">{recoveryError}</div>}
        {deletedProviders.length > 0 && (
          <div className="provider-recovery" aria-label="Provider 回收站">
            <strong>可恢复的 Provider</strong>
            {deletedProviders.map((name) => (
              <button className="btn tiny" type="button" disabled={busy} key={name} onClick={() => onRestore(name)}>
                恢复 {name}
              </button>
            ))}
          </div>
        )}
        {providers.length === 0 && (
          <div className="empty-state">
            <strong>还没有供应商</strong>
            <span>点击右上角“添加供应商”开始配置。</span>
          </div>
        )}
        {providers.map((provider) => (
          <article className={`provider-card ${managedProvider === provider.name ? "expanded" : ""}`} key={provider.name}>
            <div className="provider-card-head">
              <div className="provider-monogram" aria-hidden="true">{provider.name.slice(0, 2).toUpperCase()}</div>
              <div className="provider-main">
                <div className="provider-name">{provider.name}</div>
                <div className="provider-url">{provider.base_url}</div>
                <div className="provider-models">
                  {provider.models.slice(0, 4).map((model) => <span className="chip" key={model}>{model}</span>)}
                  {provider.models.length > 4 && <span className="chip quiet-chip">+{provider.models.length - 4}</span>}
                </div>
              </div>
              <div className="provider-side">
                <span className={`auth ${provider.has_auth ? "yes" : "no"}`}>
                  {provider.has_auth ? "Key 已就绪" : "无鉴权"}
                </span>
                <button
                  className="btn tiny"
                  type="button"
                  disabled={busy}
                  onClick={() => setManagedProvider((current) => current === provider.name ? null : provider.name)}
                >
                  {managedProvider === provider.name ? "收起" : "管理详情"}
                </button>
                <button className="btn tiny danger" type="button" disabled={busy} onClick={() => void inspectRemoval(provider.name)}>
                  删除
                </button>
              </div>
            </div>
            {managedProvider === provider.name && (
              <ProviderModelManager
                provider={provider}
                serveRunning={serveRunning}
                disabled={busy}
                onSaved={(next) => onStateChange(next, `${provider.name} 的模型已保存`)}
              />
            )}
            {removal?.name === provider.name && (
              <div className="provider-removal-preview" role="dialog" aria-label="删除影响预览">
                <strong>删除影响</strong>
                {removal.references.length > 0 ? (
                  <>
                    <p>以下路由仍在引用，必须先调整：</p>
                    <ul>{removal.references.map((reference) => <li key={reference}>{reference}</li>)}</ul>
                  </>
                ) : (
                  <p>没有路由引用。删除后会进入本地回收站，可恢复。</p>
                )}
                <div>
                  <button className="btn tiny" type="button" onClick={() => setRemoval(null)}>取消</button>
                  <button
                    className="btn tiny danger"
                    type="button"
                    disabled={busy || !removal.can_remove}
                    onClick={() => onRemove(provider.name)}
                  >
                    确认移入回收站
                  </button>
                </div>
              </div>
            )}
          </article>
        ))}
        {removalError && <div className="manager-error">{removalError}</div>}
      </div>
    </section>
  );
}
