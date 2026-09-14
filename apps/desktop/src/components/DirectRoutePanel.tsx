import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { CheckCircle2 } from "lucide-react";
import type { DirectRouteTarget, ProviderView } from "../api";
import { ProviderIcon } from "../brandIcons";
import CompactCombobox from "./CompactCombobox";
import { useErrorToast } from "./ErrorToast";
import { localizedCopy, useLocalizedCopy } from "./LanguageProvider";
import { Button } from "./ui/button";

export const DIRECT_PROVIDER_ORDER_STORAGE_KEY = "token-station-direct-provider-order-v1";

interface DirectRoutePanelProps {
  providers: ProviderView[];
  target?: DirectRouteTarget | null;
  busy: boolean;
  applying: boolean;
  agent?: boolean;
  applicationStatus?: string;
  applicationError?: string | null;
  onApply: (upstream: string, model: string) => boolean | void | Promise<boolean | void>;
  onDraftChange?: (hasUnappliedTarget: boolean) => void;
}

function storedOrder(): string[] {
  try {
    const parsed = JSON.parse(localStorage.getItem(DIRECT_PROVIDER_ORDER_STORAGE_KEY) ?? "[]");
    return Array.isArray(parsed) && parsed.every((item) => typeof item === "string") ? parsed : [];
  } catch {
    return [];
  }
}

function reconcileOrder(current: string[], providerNames: string[]) {
  const available = new Set(providerNames);
  const known = current.filter((name, index) => available.has(name) && current.indexOf(name) === index);
  const knownSet = new Set(known);
  return [...known, ...providerNames.filter((name) => !knownSet.has(name))];
}

function initialModels(providers: ProviderView[], target?: DirectRouteTarget | null) {
  return Object.fromEntries(providers.map((provider) => [
    provider.name,
    target?.upstream === provider.name
      ? (target.model && provider.models.includes(target.model) ? target.model : "")
      : (provider.models[0] ?? ""),
  ]));
}

interface DirectProviderRowProps {
  provider: ProviderView;
  model: string;
  selected: boolean;
  busy: boolean;
  onSelect: () => void;
  onModelChange: (model: string) => void;
  rowRef: (node: HTMLDivElement | null) => void;
}

function DirectProviderRow({
  provider,
  model,
  selected,
  busy,
  onSelect,
  onModelChange,
  rowRef,
}: DirectProviderRowProps) {
  const { copy } = useLocalizedCopy();
  const hasModels = provider.models.length > 0;
  const selectionLabel = copy(
    selected ? "Selected" : "Not selected",
    selected ? "已选中" : "未选中",
    selected ? "已選取" : "未選取",
    selected ? "選択済み" : "未選択",
  );
  const modelLabel = model || copy("No available models", "无可用模型", "無可用模型", "利用可能なモデルがありません");
  return (
    <div className="direct-provider-item" ref={rowRef}>
      <div
        className={`direct-provider-row${selected ? " selected" : ""}${hasModels ? "" : " unavailable"}`}
        onClick={() => {
          if (busy || !hasModels) return;
          onSelect();
        }}
      >
        <input
          className="direct-provider-radio"
          type="radio"
          name="direct-provider"
          checked={selected}
          disabled={busy || !hasModels}
          aria-label={`${modelLabel} · ${provider.name} · ${selectionLabel}`}
          onChange={onSelect}
        />
        <span className="direct-provider-brand" aria-hidden="true">
          <ProviderIcon id={provider.brand_id} label={provider.name} size={34} />
        </span>
        <span className="direct-provider-copy">
          <strong>{provider.name}</strong>
          <small>{hasModels
            ? copy("Provider", "供应商", "供應商", "プロバイダー")
            : copy("Manage a model before selecting", "请先添加已管理模型", "請先新增已管理模型", "まず管理済みモデルを追加してください")}</small>
        </span>
        <CompactCombobox
          ariaLabel={copy(`${provider.name} model`, `${provider.name} 模型`, `${provider.name} 模型`, `${provider.name} モデル`)}
          value={model}
          disabled={busy || !hasModels}
          options={provider.models.map((providerModel) => ({ value: providerModel, label: providerModel }))}
          onChange={onModelChange}
        />
        <CheckCircle2 className="direct-selected-mark" aria-hidden="true" />
      </div>
    </div>
  );
}

export default function DirectRoutePanel({
  providers,
  target = null,
  busy,
  applying,
  agent = false,
  applicationStatus,
  applicationError,
  onApply,
  onDraftChange,
}: DirectRoutePanelProps) {
  const { copy, language } = useLocalizedCopy();
  const { showError } = useErrorToast();
  const providerNamesKey = providers.map((provider) => provider.name).join("\u0000");
  const providerModelsKey = providers
    .map((provider) => `${provider.name}\u0001${provider.models.join("\u0002")}`)
    .join("\u0000");
  const providerNames = useMemo(() => providers.map((provider) => provider.name), [providerNamesKey]);
  const [providerOrder, setProviderOrder] = useState(() => reconcileOrder(storedOrder(), providerNames));
  const [selectedProvider, setSelectedProvider] = useState(() => (
    target && providerNames.includes(target.upstream) ? target.upstream : ""
  ));
  const [modelByProvider, setModelByProvider] = useState<Record<string, string>>(
    () => initialModels(providers, target),
  );
  const targetKey = target ? `${target.upstream}\u0000${target.model ?? ""}` : "";
  const previousTargetKey = useRef(targetKey);
  const onDraftChangeRef = useRef(onDraftChange);
  onDraftChangeRef.current = onDraftChange;
  const rowNodesRef = useRef(new Map<string, HTMLDivElement>());
  const pendingPositionsRef = useRef<Map<string, DOMRect> | null>(null);
  const [switchingImageModel, setSwitchingImageModel] = useState(false);

  useEffect(() => {
    setProviderOrder((current) => {
      return reconcileOrder(current, providerNames);
    });
    setModelByProvider((current) => Object.fromEntries(providers.map((provider) => {
      const currentModel = current[provider.name];
      if (currentModel && provider.models.includes(currentModel)) {
        return [provider.name, currentModel];
      }
      if (target?.upstream === provider.name) {
        return [
          provider.name,
          target.model && provider.models.includes(target.model) ? target.model : "",
        ];
      }
      return [provider.name, provider.models[0] ?? ""];
    })));
    setSelectedProvider((current) => providerNames.includes(current) ? current : "");
    // providerModelsKey is a stable content key; Provider arrays may be recreated after every IPC result.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [providerModelsKey]);

  useEffect(() => {
    try {
      localStorage.setItem(DIRECT_PROVIDER_ORDER_STORAGE_KEY, JSON.stringify(providerOrder));
    } catch {
      showError(
        localizedCopy(
          language,
          "Could not save the provider display order; the applied provider stays first for this session.",
          "无法保存供应商显示顺序；已应用的供应商会在本次使用期间保持在首位。",
          "無法儲存供應商顯示順序；已套用的供應商會在本次使用期間保持在首位。",
          "プロバイダーの表示順を保存できませんでした。適用済みのプロバイダーはこのセッション中は先頭に表示されます。",
        ),
        "direct-provider-order-storage",
      );
    }
  }, [language, providerOrder, showError]);

  useEffect(() => {
    if (previousTargetKey.current === targetKey) return;
    previousTargetKey.current = targetKey;
    if (!target || !providerNames.includes(target.upstream)) {
      setSelectedProvider("");
      return;
    }
    setSelectedProvider(target.upstream);
    setModelByProvider((current) => ({
      ...current,
      [target.upstream]: providers
        .find((provider) => provider.name === target.upstream)
        ?.models.includes(target.model ?? "")
        ? (target.model ?? "")
        : "",
    }));
    // Equivalent provider arrays must not overwrite an un-applied model draft.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [providerModelsKey, targetKey]);

  const orderedProviders = providerOrder
    .map((name) => providers.find((provider) => provider.name === name))
    .filter((provider): provider is ProviderView => Boolean(provider));

  useLayoutEffect(() => {
    const previousPositions = pendingPositionsRef.current;
    pendingPositionsRef.current = null;
    if (!previousPositions || window.matchMedia?.("(prefers-reduced-motion: reduce)").matches) return;

    const movedRows: HTMLDivElement[] = [];
    rowNodesRef.current.forEach((node, name) => {
      const previous = previousPositions.get(name);
      if (!previous) return;
      const current = node.getBoundingClientRect();
      const offsetX = previous.left - current.left;
      const offsetY = previous.top - current.top;
      if (offsetX === 0 && offsetY === 0) return;
      node.style.transition = "none";
      node.style.transform = `translate(${offsetX}px, ${offsetY}px)`;
      movedRows.push(node);
    });
    if (movedRows.length === 0) return;
    void movedRows[0].offsetHeight;
    const frame = requestAnimationFrame(() => {
      movedRows.forEach((node) => {
        node.style.transition = "";
        node.style.transform = "";
      });
    });
    return () => {
      cancelAnimationFrame(frame);
      movedRows.forEach((node) => {
        node.style.transition = "";
        node.style.transform = "";
      });
    };
  }, [providerOrder]);

  const selectedModel = selectedProvider ? modelByProvider[selectedProvider] ?? "" : "";
  const selectedTargetValid = providers.some((provider) => (
    provider.name === selectedProvider && provider.models.includes(selectedModel)
  ));
  const hasUnappliedTarget = Boolean(selectedTargetValid && (
    !target || selectedProvider !== target.upstream || selectedModel !== (target.model ?? "")
  ));

  const appliedProvider = providers.find((provider) => provider.name === target?.upstream);
  const appliedVision = appliedProvider?.model_capabilities?.find((capability) => capability.model === target?.model)?.vision;
  const imageAlternative = target?.model && !hasUnappliedTarget && !appliedProvider?.managed_route && appliedVision !== "verified"
    ? appliedProvider?.models.find((model) => model !== target.model
      && appliedProvider.model_capabilities?.some((capability) => capability.model === model && capability.vision === "verified"))
    : undefined;

  const switchImageModel = async () => {
    if (!target || !imageAlternative || busy || applying || switchingImageModel) return;
    setSwitchingImageModel(true);
    try {
      const applied = await onApply(target.upstream, imageAlternative);
      if (applied === false) return;
      setSelectedProvider(target.upstream);
      setModelByProvider((current) => ({ ...current, [target.upstream]: imageAlternative }));

    } catch (error) {
      showError(error instanceof Error ? error.message : String(error), "direct-image-switch");
    } finally {
      setSwitchingImageModel(false);
    }
  };

  useEffect(() => {
    onDraftChange?.(hasUnappliedTarget);
  }, [hasUnappliedTarget, onDraftChange]);

  useEffect(() => () => {
    onDraftChangeRef.current?.(false);
  }, []);

  const applySelectedTarget = async () => {
    const appliedProvider = selectedProvider;
    const appliedModel = selectedModel;
    const applied = await onApply(appliedProvider, appliedModel);
    if (applied === false) return;
    const promoted = providerOrder.indexOf(appliedProvider) > 0;
    if (promoted) {
      pendingPositionsRef.current = new Map(Array.from(rowNodesRef.current, ([name, node]) => (
        [name, node.getBoundingClientRect()]
      )));
      setProviderOrder((current) => [
        appliedProvider,
        ...current.filter((name) => name !== appliedProvider),
      ]);
    }

  };

  return (
    <section
      className="panel direct-route-panel"
      aria-label={copy("Direct routing configuration", "简单路由配置", "簡單路由設定", "シンプルルーティングの設定")}
      data-onboarding-target="route-config"
    >
      <div className="panel-head split-heading direct-route-heading">
        <div>
          <h2>{copy("Direct routing", "简单路由", "簡單路由", "シンプルルーティング")}</h2>
          <p className="sub">{copy(
            agent
              ? "Send this Agent to exactly one provider and managed model."
              : "Send every request to exactly one provider and managed model.",
            agent
              ? "将当前客户端的请求固定发送给一个供应商及其已管理模型。"
              : "将请求固定发送给你明确选择的一个供应商及其已管理模型。",
            agent
              ? "將目前 Agent 的請求固定傳送至一個供應商及其已管理模型。"
              : "將所有請求固定傳送至一個供應商及其已管理模型。",
            agent
              ? "この Agent のリクエストを、1 つのプロバイダーと管理対象モデルに固定します。"
              : "すべてのリクエストを、1 つのプロバイダーと管理対象モデルに固定します。",
          )}</p>
        </div>
        <div className="direct-route-heading-actions">
          {target && (
            <>
              <span className={`direct-applied-target${hasUnappliedTarget ? " is-draft" : ""}`}>
                {hasUnappliedTarget
                  ? copy("Changes not applied", "更改未应用", "變更未套用", "変更は未適用")
                  : target.model
                    ? applying
                      ? copy("Applying…", "应用中…", "應用中…", "適用中…")
                      : applicationStatus ?? copy("Configured", "已配置", "已設定", "設定済み")
                    : copy("Incomplete", "配置未完成", "未完成", "未完成")}
              </span>
              {hasUnappliedTarget && target.model && (
                <span className="direct-applied-detail">
                  {copy("Configured target: ", "当前配置：", "目前設定：", "設定した対象：")}{target.upstream} / {target.model}
                </span>
              )}
            </>
          )}
          <Button
            type="button"
            data-onboarding-target="route-apply"
            disabled={busy || applying || switchingImageModel || !selectedTargetValid}
            onClick={() => void applySelectedTarget()}
          >
            {applying ? copy("Applying…", "应用中…", "應用中…", "適用中…") : copy("Apply", "应用", "應用", "適用")}
          </Button>
        </div>
      </div>

      {imageAlternative && target && (
        <div className="panel-foot direct-image-switch">
          <p className="sub">{copy(
            `Need images? ${imageAlternative} has passed an image check on ${target.upstream}. Switching applies it to text and images, at its own pricing.`,
            `需要看图？同一通道的 ${imageAlternative} 已通过看图验证。切换后，文字和图片请求都会使用该模型，并按该模型计费。`,
            `需要看圖？同一通道的 ${imageAlternative} 已通過看圖驗證。切換後，文字和圖片請求都會使用該模型，並按該模型計費。`,
            `画像を使いますか？同じチャネルの ${imageAlternative} は画像検証済みです。切替後はテキストと画像の両方に適用され、そのモデルの料金になります。`,
          )}</p>
          <Button type="button" variant="outline" disabled={busy || applying || switchingImageModel} onClick={() => void switchImageModel()}>
            {copy(`Switch to image model ${imageAlternative}`, `切换到看图模型 ${imageAlternative}`, `切換到看圖模型 ${imageAlternative}`, `画像対応モデル ${imageAlternative} に切替`)}
          </Button>
        </div>
      )}

      {providers.length === 0 ? (
        <p className="direct-route-empty">{copy(
          "Add a provider and manage at least one model before using Direct routing.",
          "请先添加供应商并管理至少一个模型。", "請先新增供應商並管理至少一個模型。", "シンプルルーティングを使用する前に、プロバイダーを追加し、少なくとも1つのモデルを管理してください。"
        )}</p>
      ) : (
        <div className="direct-provider-list" role="radiogroup" aria-label={copy("Direct provider", "简单路由供应商", "簡單路由供應商", "シンプルルーティングプロバイダー")}>
          {orderedProviders.map((provider) => (
            <DirectProviderRow
              key={provider.name}
              provider={provider}
              model={modelByProvider[provider.name] ?? ""}
              selected={selectedProvider === provider.name}
              busy={busy || applying || switchingImageModel}
              onSelect={() => setSelectedProvider(provider.name)}
              onModelChange={(nextModel) => setModelByProvider((current) => ({
                ...current,
                [provider.name]: nextModel,
              }))}
              rowRef={(node) => {
                if (node) rowNodesRef.current.set(provider.name, node);
                else rowNodesRef.current.delete(provider.name);
              }}
            />
          ))}
        </div>
      )}

      {applicationError && (
        <p className="panel-foot" role="alert">{applicationError}</p>
      )}

      {!selectedTargetValid && (
        <footer className="panel-foot direct-route-actions">
          <span className="foot-hint">{copy(
            target?.upstream === selectedProvider && !target.model
              ? `Provider ${target.upstream} was preserved; select a model, then apply.`
              : "Select a provider with an available model, then apply.",
            target?.upstream === selectedProvider && !target.model
              ? `已保留供应商 ${target.upstream}；请选择模型后再应用。`
              : "请选择一个有可用模型的供应商，再点击应用。",
            target?.upstream === selectedProvider && !target.model
              ? `已保留供應商 ${target.upstream}；請選取模型後再套用。`
              : "請選取具有可用模型的供應商，然後再套用。",
            target?.upstream === selectedProvider && !target.model
              ? `プロバイダー ${target.upstream} を保持しました。モデルを選択してから適用してください。`
              : "利用可能なモデルがあるプロバイダーを選択してから適用してください。",
          )}</span>
        </footer>
      )}
    </section>
  );
}
