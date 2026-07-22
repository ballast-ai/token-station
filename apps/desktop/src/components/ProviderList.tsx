import { useState } from "react";
import type { ProviderView, StateView } from "../api";
import ProviderModelManager from "./ProviderModelManager";

interface ProviderListProps {
  providers: ProviderView[];
  serveRunning: boolean;
  busy: boolean;
  onRemove: (name: string) => void;
  onStateChange: (state: StateView, message: string) => void;
}

function windowLabel(tokens: number): string {
  if (tokens <= 0) return "未知";
  if (tokens >= 1000) return `${Math.round(tokens / 1000)}k`;
  return String(tokens);
}

export default function ProviderList({
  providers,
  serveRunning,
  busy,
  onRemove,
  onStateChange,
}: ProviderListProps) {
  const [managedProvider, setManagedProvider] = useState<string | null>(null);
  const [detailProvider, setDetailProvider] = useState<string | null>(null);

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
                  onClick={() => setDetailProvider((current) => current === provider.name ? null : provider.name)}
                >
                  {detailProvider === provider.name ? "收起能力" : "能力"}
                </button>
                <button
                  className="btn tiny"
                  type="button"
                  disabled={busy}
                  onClick={() => setManagedProvider((current) => current === provider.name ? null : provider.name)}
                >
                  {managedProvider === provider.name ? "收起" : "管理模型"}
                </button>
                <button className="btn tiny danger" type="button" disabled={busy} onClick={() => onRemove(provider.name)}>
                  删除
                </button>
              </div>
            </div>
            {detailProvider === provider.name && (
              <div className="provider-capabilities">
                {(provider.model_details ?? []).length === 0 ? (
                  <p className="empty-note">该供应商还没有模型，或未申报能力。</p>
                ) : (
                  <table className="capability-table">
                    <thead>
                      <tr>
                        <th>模型</th>
                        <th title="是否支持函数/工具调用">工具</th>
                        <th title="是否支持图像输入">视觉</th>
                        <th title="是否支持严格 JSON Schema 结构化输出">JSON</th>
                        <th title="最大上下文窗口(输入+输出),未知则路由不往此发长上下文">上下文</th>
                      </tr>
                    </thead>
                    <tbody>
                      {(provider.model_details ?? []).map((cap) => (
                        <tr key={cap.model}>
                          <td className="mono">{cap.model}</td>
                          <td className={cap.tool ? "cap-yes" : "cap-no"}>{cap.tool ? "✓" : "—"}</td>
                          <td className={cap.vision ? "cap-yes" : "cap-no"}>{cap.vision ? "✓" : "—"}</td>
                          <td className={cap.json_schema ? "cap-yes" : "cap-no"}>{cap.json_schema ? "✓" : "—"}</td>
                          <td className={cap.context_window > 0 ? "" : "cap-no"}>{windowLabel(cap.context_window)}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                )}
              </div>
            )}
            {managedProvider === provider.name && (
              <ProviderModelManager
                provider={provider}
                serveRunning={serveRunning}
                disabled={busy}
                onSaved={(next) => onStateChange(next, `${provider.name} 的模型已保存`)}
              />
            )}
          </article>
        ))}
      </div>
    </section>
  );
}
