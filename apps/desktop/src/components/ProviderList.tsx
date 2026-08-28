import { useRef, useState } from "react";
import { previewProviderRemoval } from "../api";
import type { ProviderRemovalPreview, ProviderView, StateView } from "../api";
import { providerDisplayName } from "../providerPresentation";
import ProviderModelManager from "./ProviderModelManager";
import { useLocalizedCopy } from "./LanguageProvider";
import { humanizeAppError } from "../errors";
import { ProviderIcon } from "../brandIcons";
import { useErrorToast } from "./ErrorToast";
import { Button } from "./ui/button";
import { Badge } from "./ui/badge";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "./ui/dialog";

interface ProviderListProps {
  providers: ProviderView[];
  deletedProviders: string[];
  recoveryError: string | null;
  serveRunning: boolean;
  busy: boolean;
  onRemove: (name: string) => Promise<boolean>;
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
  const [removing, setRemoving] = useState(false);
  const providerListRef = useRef<HTMLElement>(null);
  const manageTriggerRef = useRef<HTMLButtonElement | null>(null);
  const removalTriggerRef = useRef<HTMLButtonElement | null>(null);
  const removalRequestRef = useRef(0);
  const modelCount = providers.reduce((total, provider) => total + provider.models.length, 0);
  const activeProvider = providers.find((provider) => provider.name === managedProvider);

  const inspectRemoval = async (name: string) => {
    const request = ++removalRequestRef.current;
    setManagedProvider(null);
    try {
      setRemoval(null);
      const preview = await previewProviderRemoval(name);
      if (request === removalRequestRef.current) setRemoval(preview);
    } catch (caught) {
      if (request === removalRequestRef.current) {
        showError(humanizeAppError(caught, language), `provider-removal-preview:${name}`);
      }
    }
  };

  const restoreFocus = (trigger: HTMLButtonElement | null) => {
    if (trigger?.isConnected) trigger.focus();
    else providerListRef.current?.focus();
  };

  return (
    <section ref={providerListRef} className="panel provider-panel" data-surface="flat-model-collection" tabIndex={-1}>
      <div className="panel-head split-heading">
        <div>
          <h2>{copy("Managed models", "已接入模型", "已接入模型", "接続済みモデル")}</h2>
        </div>
        <Badge variant="secondary" className="count-badge provider-total-badge">
          {copy(`${modelCount} models`, `${modelCount} 个模型`, `${modelCount} 個模型`, `${modelCount} 個モデル`)}
        </Badge>
      </div>

      <div className="provider-list">
        {recoveryError && (
          <div className="manager-error">{humanizeAppError(recoveryError, language)}</div>
        )}
        {deletedProviders.length > 0 && (
          <div className="provider-recovery" aria-label={copy("Provider recycle bin", "Provider 回收站", "供應商回收站", "プロバイダーごみ箱")}>
            <strong>{copy("Recoverable providers", "可恢复的 Provider", "可恢復的供應商", "復元可能なプロバイダー")}</strong>
            {deletedProviders.map((name) => (
              <Button variant="secondary" size="sm" type="button" disabled={busy} key={name} onClick={() => onRestore(name)}>
                {copy(`Restore ${name}`, `恢复 ${name}`, `恢復 ${name}`, `${name} を復元`)}
              </Button>
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
        {providers.map((provider) => {
          const displayName = providerDisplayName(provider);
          return (
            <article
              className="provider-card"
              data-surface="flat-color-block"
              key={provider.name}
              role="group"
              aria-label={copy(`${displayName} provider`, `${displayName} 供应商`, `${displayName} 供應商`, `${displayName} プロバイダー`)}
            >
              <div className="provider-card-head provider-identity-bar">
                <div className="provider-monogram" aria-hidden="true">
                  <ProviderIcon id={provider.brand_id} label={displayName} size={34} />
                </div>
                <div className="provider-main provider-identity">
                  <small>{copy("Provider", "供应商", "供應商", "プロバイダー")}</small>
                  <strong className="provider-identity-name">{displayName}</strong>
                  <div className="provider-url">{provider.base_url}</div>
                </div>
              <Badge variant="secondary" className="provider-model-count">
                {copy(`${provider.models.length} models`, `${provider.models.length} 个模型`, `${provider.models.length} 個模型`, `${provider.models.length} 個モデル`)}
              </Badge>
              <div className="provider-side">
                {provider.access_tier === "free" && (
                  <Badge variant="secondary" className="provider-access-badge">{copy("Free", "免费", "免費", "無料")}</Badge>
                )}
                <Badge variant={provider.has_auth ? "secondary" : "ghost"} className={`auth ${provider.has_auth ? "yes" : "no"}`}>
                  {provider.has_auth
                    ? copy(
                      "Credential ready",
                      "凭据已配置", "憑據已設定", "認証情報設定済み"
                    )
                    : provider.credential_source === "none"
                      ? copy("No credential required", "无需凭据", "無需憑據", "認証情報は不要")
                      : copy("Credential missing", "缺少凭据", "缺少憑據", "認証情報がありません")}
                </Badge>
                <Button
                  variant="secondary"
                  size="sm"
                  type="button"
                  disabled={busy}
                  onClick={(event) => {
                    removalRequestRef.current += 1;
                    setRemoval(null);
                    manageTriggerRef.current = event.currentTarget;
                    setManagedProvider(provider.name);
                  }}
                >
                  {copy("Manage", "管理", "管理", "管理")}
                </Button>
                <Button
                  variant="destructive"
                  size="sm"
                  type="button"
                  disabled={busy}
                  onClick={(event) => {
                    removalTriggerRef.current = event.currentTarget;
                    void inspectRemoval(provider.name);
                  }}
                >
                  {copy("Delete", "删除", "刪除", "削除")}
                </Button>
              </div>
            </div>
            <ul
              className="provider-primary-models"
              role="list"
              aria-label={copy(`${displayName} models`, `${displayName} 模型`, `${displayName} 模型`, `${displayName} モデル`)}
              data-layout="compact-three-column"
              data-surface="plain-model-grid"
            >
              {provider.models.length > 0 ? provider.models.map((model) => (
                <li role="listitem" key={model} title={`${model} · ${displayName}`}>
                  <strong>{model}</strong>
                </li>
              )) : (
                <li role="listitem">
                  <strong>{copy("No managed models", "暂无已管理模型", "目前無已管理模型", "現在管理されていないモデルがあります")}</strong>
                </li>
              )}
              </ul>
            </article>
          );
        })}
      </div>

      <Dialog open={Boolean(activeProvider)} onOpenChange={(open) => !open && setManagedProvider(null)}>
        {activeProvider && (
          <DialogContent
            className="provider-management-dialog"
            closeLabel={copy("Close", "关闭", "關閉", "閉じる")}
            onCloseAutoFocus={(event) => {
              event.preventDefault();
              restoreFocus(manageTriggerRef.current);
            }}
          >
            <DialogHeader>
              <DialogTitle>{copy(`Manage ${activeProvider.name}`, `管理 ${activeProvider.name}`, `管理 ${activeProvider.name}`, `${activeProvider.name} を管理`)}</DialogTitle>
              <DialogDescription>{copy(
                "Manage models, credentials, and connection diagnostics without changing the model list behind the dialog.",
                "在弹窗中管理模型、凭据与连接诊断，不改变后方模型列表的位置。",
                "在彈窗中管理模型、憑證與連線診斷，不改變後方模型清單的位置。",
                "ダイアログ内でモデル、認証情報、接続診断を管理します。",
              )}</DialogDescription>
            </DialogHeader>
            <div className="provider-management-dialog-body">
              {activeProvider.access_tier === "free" ? (
                <div className="free-provider-managed-note">
                  {copy(
                    "Free provider endpoints and model sets are protected by the catalog. To change the key or models, delete this provider and add it again through the free catalog.",
                    "免费实例的端点与模型集合受免费目录保护。需要更换 Key 或模型时，请删除后从免费目录重新验证添加。", "免費例項的端點與模型集合受免費目錄保護。需要更換 Key 或模型時，請刪除後從免費目錄重新驗證新增。", "無料インスタンスのエンドポイントとモデルセットは無料カタログによって保護されています。Keyやモデルを変更する場合は、このプロバイダーを削除し、無料カタログから再度追加してください。"
                  )}
                </div>
              ) : (
                <ProviderModelManager
                  provider={activeProvider}
                  serveRunning={serveRunning}
                  disabled={busy}
                  onSaved={onStateChange}
                />
              )}
            </div>
          </DialogContent>
        )}
      </Dialog>

      <Dialog open={Boolean(removal)} onOpenChange={(open) => !open && setRemoval(null)}>
        {removal && (
          <DialogContent
            className="provider-removal-dialog"
            closeLabel={copy("Close", "关闭", "關閉", "閉じる")}
            onCloseAutoFocus={(event) => {
              event.preventDefault();
              restoreFocus(removalTriggerRef.current);
            }}
          >
            <DialogHeader>
              <DialogTitle>{copy("Deletion impact preview", "删除影响预览", "刪除影響預覽", "削除影響プレビュー")}</DialogTitle>
              <DialogDescription>{copy(
                `Review every route that references ${removal.name} before removal.`,
                `删除 ${removal.name} 前，请检查所有引用它的路由。`,
                `刪除 ${removal.name} 前，請檢查所有引用它的路由。`,
                `${removal.name} を削除する前に、参照するすべてのルートを確認してください。`,
              )}</DialogDescription>
            </DialogHeader>
            <div className="provider-removal-preview">
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
            </div>
            <DialogFooter>
              <Button variant="outline" type="button" onClick={() => setRemoval(null)}>
                {copy("Cancel", "取消", "取消", "キャンセル")}
              </Button>
              <Button
                variant="destructive"
                type="button"
                disabled={busy || removing || !removal.can_remove}
                onClick={async () => {
                  const name = removal.name;
                  setRemoving(true);
                  try {
                    if (await onRemove(name)) setRemoval(null);
                  } finally {
                    setRemoving(false);
                  }
                }}
              >
                {copy("Move to recycle bin", "确认移入回收站", "確認移入回收站", "ゴミ箱に移動")}
              </Button>
            </DialogFooter>
          </DialogContent>
        )}
      </Dialog>
    </section>
  );
}
