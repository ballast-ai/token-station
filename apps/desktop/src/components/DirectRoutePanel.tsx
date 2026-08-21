import {
  DndContext,
  KeyboardSensor,
  PointerSensor,
  closestCorners,
  pointerWithin,
  useSensor,
  useSensors,
  type Announcements,
  type CollisionDetection,
  type DragEndEvent,
  type Modifier,
  type ScreenReaderInstructions,
} from "@dnd-kit/core";
import {
  SortableContext,
  arrayMove,
  sortableKeyboardCoordinates,
  useSortable,
  verticalListSortingStrategy,
} from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import { useEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { CheckCircle2, GripVertical } from "lucide-react";
import type { DirectRouteTarget, ProviderView } from "../api";
import { ProviderIcon } from "../brandIcons";
import CompactCombobox from "./CompactCombobox";
import { useErrorToast } from "./ErrorToast";
import { localizedCopy, useLocalizedCopy } from "./LanguageProvider";

export const DIRECT_PROVIDER_ORDER_STORAGE_KEY = "token-station-direct-provider-order-v1";

interface DirectRoutePanelProps {
  providers: ProviderView[];
  target?: DirectRouteTarget | null;
  busy: boolean;
  applying: boolean;
  agent?: boolean;
  onApply: (upstream: string, model: string) => void | Promise<void>;
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

const restrictToVerticalAxis: Modifier = ({ transform }) => ({
  ...transform,
  x: 0,
});

const directProviderCollisionDetection: CollisionDetection = (args) => {
  if (args.pointerCoordinates) return pointerWithin(args);
  return closestCorners(args);
};

interface SortableDirectProviderRowProps {
  provider: ProviderView;
  index: number;
  model: string;
  selected: boolean;
  busy: boolean;
  onSelect: () => void;
  onMove: (targetIndex: number) => void;
  onModelChange: (model: string) => void;
}

function SortableDirectProviderRow({
  provider,
  index,
  model,
  selected,
  busy,
  onSelect,
  onMove,
  onModelChange,
}: SortableDirectProviderRowProps) {
  const { copy } = useLocalizedCopy();
  const {
    attributes,
    listeners,
    setNodeRef,
    setActivatorNodeRef,
    transform,
    transition,
    isDragging,
  } = useSortable({
    id: provider.name,
    disabled: busy,
    animateLayoutChanges: () => false,
    attributes: { roleDescription: copy("sortable item", "可排序项", "可排序專案", "並べ替え可能な項目") },
  });
  const hasModels = provider.models.length > 0;
  const selectionLabel = copy(
    selected ? "Selected" : "Not selected",
    selected ? "已选中" : "未选中",
    selected ? "已選取" : "未選取",
    selected ? "選択済み" : "未選択",
  );
  const modelLabel = model || copy("No available models", "无可用模型", "無可用模型", "利用可能なモデルがありません");
  const sortableStyle = {
    transform: CSS.Transform.toString(transform),
    transition,
  };

  const handleSortKeyDown = (event: KeyboardEvent<HTMLButtonElement>) => {
    if (!isDragging && (event.key === "ArrowUp" || event.key === "ArrowDown")) {
      event.preventDefault();
      onMove(index + (event.key === "ArrowUp" ? -1 : 1));
      return;
    }
    listeners?.onKeyDown?.(event);
  };

  return (
    <div
      className={`direct-provider-sortable${isDragging ? " dragging" : ""}`}
      ref={setNodeRef}
      style={sortableStyle}
    >
      <div
        className={`direct-provider-row${selected ? " selected" : ""}${hasModels ? "" : " unavailable"}${isDragging ? " dragging" : ""}`}
        onClick={(event) => {
          if (busy || !hasModels || (event.target as HTMLElement).closest(".direct-drag-handle")) return;
          onSelect();
        }}
      >
        <button
          className="direct-drag-handle"
          type="button"
          ref={setActivatorNodeRef}
          disabled={busy}
          {...attributes}
          {...listeners}
          aria-label={copy(
            `Reorder ${provider.name}; position ${index + 1}; use the up or down arrow key`,
            `调整 ${provider.name} 顺序；当前第 ${index + 1} 项；使用上下方向键`, `調整 ${provider.name} 順序；當前第 ${index + 1} 項；使用上下方向鍵`, `${provider.name} の順序を調整；現在の位置は ${index + 1} 番；上または下の矢印キーを使用`
          )}
          onKeyDown={handleSortKeyDown}
        >
          <GripVertical aria-hidden="true" />
        </button>
        <input
          type="radio"
          name="direct-provider"
          checked={selected}
          disabled={busy || !hasModels}
          aria-label={`${modelLabel} · ${provider.name} · ${selectionLabel}`}
          onChange={onSelect}
        />
        <CompactCombobox
          ariaLabel={copy(`${provider.name} model`, `${provider.name} 模型`, `${provider.name} 模型`, `${provider.name} モデル`)}
          value={model}
          disabled={busy || !hasModels}
          options={provider.models.map((providerModel) => ({ value: providerModel, label: providerModel }))}
          onChange={onModelChange}
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
  onApply,
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
  const [sortAnnouncement, setSortAnnouncement] = useState("");
  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 8 } }),
    useSensor(KeyboardSensor, { coordinateGetter: sortableKeyboardCoordinates }),
  );

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
          "Could not save the provider display order; reordering still works for this session.",
          "无法保存供应商显示顺序；本次排序仍可使用。",
          "無法儲存供應商顯示順序；本次工作階段仍可重新排序。",
          "プロバイダーの表示順を保存できませんでした。このセッションでは並べ替えを引き続き使用できます。",
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

  const moveProvider = (name: string, targetIndex: number) => {
    const fromIndex = providerOrder.indexOf(name);
    if (fromIndex < 0) return;
    if (targetIndex < 0 || targetIndex >= providerOrder.length) {
      setSortAnnouncement(copy(
        `${name} is already at the ${targetIndex < 0 ? "top" : "bottom"} of the list.`,
        `${name} 已在列表${targetIndex < 0 ? "顶部" : "底部"}。`,
        `${name} 已在列表${targetIndex < 0 ? "頂端" : "底部"}。`,
        `${name} はリストの${targetIndex < 0 ? "先頭" : "末尾"}にあります。`,
      ));
      return;
    }
    if (fromIndex === targetIndex) return;
    setProviderOrder(arrayMove(providerOrder, fromIndex, targetIndex));
    setSortAnnouncement(copy(
      `Moved ${name} to position ${targetIndex + 1} of ${providerOrder.length}.`,
      `已将 ${name} 移到第 ${targetIndex + 1} 项，共 ${providerOrder.length} 项。`, `已將 ${name} 移到第 ${targetIndex + 1} 項，共 ${providerOrder.length} 項。`, `${name} を ${providerOrder.length} 項中の ${targetIndex + 1} 項に移動しました。`
    ));
  };

  const finishProviderDrag = ({ active, over }: DragEndEvent) => {
    if (!over || active.id === over.id) return;
    setProviderOrder((current) => {
      const fromIndex = current.indexOf(String(active.id));
      const targetIndex = current.indexOf(String(over.id));
      if (fromIndex < 0 || targetIndex < 0 || fromIndex === targetIndex) return current;
      return arrayMove(current, fromIndex, targetIndex);
    });
  };

  const providerPosition = (id: string | number) => providerOrder.indexOf(String(id)) + 1;
  const screenReaderInstructions: ScreenReaderInstructions = {
    draggable: copy(
      "Press Space or Enter to pick up this provider. Use the up and down arrow keys to move it, then press Space or Enter to drop it. Press Escape to cancel.",
      "按空格或回车拾取该供应商，使用上下方向键移动，再按空格或回车放下；按 Escape 取消。", "按空白鍵或 Enter 鍵選取該供應商，使用上下方向鍵移動，再按空白鍵或 Enter 鍵放下；按 Escape 鍵取消。", "スペースキーまたは Enter キーでこのプロバイダーを選択し、上下矢印キーで移動して、もう一度スペースキーまたは Enter キーで配置します。Escape キーでキャンセルします。"
    ),
  };
  const announcements: Announcements = {
    onDragStart: ({ active }) => copy(
      `Picked up ${String(active.id)}, position ${providerPosition(active.id)} of ${providerOrder.length}.`,
      `已拾取 ${String(active.id)}，当前第 ${providerPosition(active.id)} 项，共 ${providerOrder.length} 项。`, `已拾取 ${String(active.id)}，當前第 ${providerPosition(active.id)} 項，共 ${providerOrder.length} 項。`, `${String(active.id)} を選択しました。現在 ${providerOrder.length} 項中の ${providerPosition(active.id)} 項です。`
    ),
    onDragOver: ({ active, over }) => over ? copy(
      `${String(active.id)} is over position ${providerPosition(over.id)} of ${providerOrder.length}.`,
      `${String(active.id)} 当前位于第 ${providerPosition(over.id)} 项，共 ${providerOrder.length} 项。`, `${String(active.id)} 當前位於第 ${providerPosition(over.id)} 項，共 ${providerOrder.length} 項。`, `${String(active.id)} は ${providerOrder.length} 項中の ${providerPosition(over.id)} 項に現在位置しています。`
    ) : copy(
      `${String(active.id)} is outside the provider list.`,
      `${String(active.id)} 已移出供应商列表。`, `${String(active.id)} 已移出供應商列表。`, `${String(active.id)} はプロバイダー一覧から外れました。`
    ),
    onDragEnd: ({ active, over }) => over ? copy(
      `Dropped ${String(active.id)} at position ${providerPosition(over.id)} of ${providerOrder.length}.`,
      `已将 ${String(active.id)} 放到第 ${providerPosition(over.id)} 项，共 ${providerOrder.length} 项。`, `已將 ${String(active.id)} 放到第 ${providerPosition(over.id)} 項，共 ${providerOrder.length} 項。`, `${String(active.id)} を ${providerOrder.length} 項中の ${providerPosition(over.id)} 項に配置しました。`
    ) : copy(
      `Sorting cancelled. ${String(active.id)} kept its position.`,
      `已取消排序，${String(active.id)} 保持原位置。`, `已取消排序，${String(active.id)} 保持原位置。`, `並べ替えをキャンセルしました。${String(active.id)} は元の位置を維持します。`
    ),
    onDragCancel: ({ active }) => copy(
      `Sorting cancelled. ${String(active.id)} kept its position.`,
      `已取消排序，${String(active.id)} 保持原位置。`, `已取消排序，${String(active.id)} 保持原位置。`, `並べ替えをキャンセルしました。${String(active.id)} は元の位置を維持します。`
    ),
  };

  const selectedModel = selectedProvider ? modelByProvider[selectedProvider] ?? "" : "";
  const selectedTargetValid = providers.some((provider) => (
    provider.name === selectedProvider && provider.models.includes(selectedModel)
  ));

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
            <span className="direct-applied-target">
              {target.model
                ? <>{copy("Applied", "已应用", "已應用", "適用済み")} · {target.upstream} / {target.model}</>
                : <>{copy("Incomplete", "配置未完成", "未完成", "未完成")} · {target.upstream} / {copy("Select a model", "待选择模型", "待選擇模型", "選択するモデル")}</>}
            </span>
          )}
          <button
            className="btn primary"
            type="button"
            data-onboarding-target="route-apply"
            disabled={busy || applying || !selectedTargetValid}
            onClick={() => void onApply(selectedProvider, selectedModel)}
          >
            {applying ? copy("Applying…", "应用中…", "應用中…", "適用中…") : copy("Apply", "应用", "應用", "適用")}
          </button>
        </div>
      </div>

      {providers.length === 0 ? (
        <p className="direct-route-empty">{copy(
          "Add a provider and manage at least one model before using Direct routing.",
          "请先添加供应商并管理至少一个模型。", "請先新增供應商並管理至少一個模型。", "シンプルルーティングを使用する前に、プロバイダーを追加し、少なくとも1つのモデルを管理してください。"
        )}</p>
      ) : (
        <DndContext
          sensors={sensors}
          collisionDetection={directProviderCollisionDetection}
          modifiers={[restrictToVerticalAxis]}
          accessibility={{ announcements, screenReaderInstructions }}
          onDragEnd={finishProviderDrag}
        >
          <SortableContext
            items={orderedProviders.map((provider) => provider.name)}
            strategy={verticalListSortingStrategy}
          >
            <div className="direct-provider-list" role="radiogroup" aria-label={copy("Direct provider", "简单路由供应商", "簡單路由供應商", "シンプルルーティングプロバイダー")}>
              {orderedProviders.map((provider, index) => (
                <SortableDirectProviderRow
                  key={provider.name}
                  provider={provider}
                  index={index}
                  model={modelByProvider[provider.name] ?? ""}
                  selected={selectedProvider === provider.name}
                  busy={busy}
                  onSelect={() => setSelectedProvider(provider.name)}
                  onMove={(targetIndex) => moveProvider(provider.name, targetIndex)}
                  onModelChange={(nextModel) => setModelByProvider((current) => ({
                    ...current,
                    [provider.name]: nextModel,
                  }))}
                />
              ))}
            </div>
          </SortableContext>
        </DndContext>
      )}

      <span className="sr-only" role="status" aria-live="polite" aria-atomic="true">
        {sortAnnouncement}
      </span>

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
