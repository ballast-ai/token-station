import type {
  AgentId,
  HarnessModelTarget,
  ProviderView,
} from "../api";
import { ProviderIcon } from "../brandIcons";
import CompactCombobox, { type CompactComboboxOption } from "./CompactCombobox";
import { useLocalizedCopy } from "./LanguageProvider";

interface HarnessModelMappingProps {
  agentId: AgentId;
  providers: ProviderView[];
  routes?: Record<string, HarnessModelTarget>;
  readOnly?: boolean;
  disabled?: boolean;
  saveDisabled?: boolean;
  error?: string | null;
  onChange?: (requestedModel: string, target: HarnessModelTarget) => void | Promise<void>;
  onSave?: () => void | Promise<void>;
}

interface MappingRow {
  label: string;
  requestedModel: string;
}

export default function HarnessModelMapping({
  agentId,
  providers,
  routes,
  readOnly = false,
  disabled = false,
  saveDisabled = false,
  error,
  onChange,
  onSave,
}: HarnessModelMappingProps) {
  const { copy } = useLocalizedCopy();
  const rows: MappingRow[] = agentId === "claude-code"
    ? [
      { label: "Haiku", requestedModel: "fast" },
      { label: "Sonnet", requestedModel: "balanced" },
      { label: "Opus", requestedModel: "power" },
      { label: "Fable", requestedModel: "claude-fable-5-1" },
    ]
    : agentId === "opencode"
      ? [
        { label: "auto", requestedModel: "auto" },
        { label: "fast", requestedModel: "fast" },
        { label: "balanced", requestedModel: "balanced" },
        { label: "power", requestedModel: "power" },
      ]
      : [];
  if (rows.length === 0) return null;

  const controlsDisabled = disabled || readOnly;
  const staleSuffix = copy(
    " (unavailable — reselect)",
    "（已失效·请重选）",
    "（已失效·請重選）",
    "（無効·再度選択）",
  );
  const notSelected = copy("Not selected", "未选择", "未選擇", "選択されていません");

  return (
    <section className="harness-model-mapping" aria-label={copy(
      "Harness model mapping",
      "Harness 模型映射",
      "Harness 模型映射",
      "Harness モデルマッピング",
    )}>
      <div className="harness-model-mapping-head">
        <div>
          <h3>{copy("Harness model mapping", "Harness 模型映射", "Harness 模型映射", "Harness モデルマッピング")}</h3>
          <p>{readOnly
            ? copy(
              "Inherited mapping. Set independent routing to edit it.",
              "当前继承全局映射。设置独立路由后可编辑。",
              "目前繼承全域映射。設定獨立路由後可編輯。",
              "継承されたマッピングです。独立ルーティングにすると編集できます。",
            )
            : copy(
              "Choose a provider and model for every request. Save and restart to apply.",
              "为每种请求选择供应商和模型，保存并重启后生效。",
              "為每種請求選擇供應商和模型，儲存並重新啟動後生效。",
              "各リクエストのプロバイダーとモデルを選び、保存して再起動すると適用されます。",
            )}</p>
        </div>
        {onSave ? (
          <button className="btn primary" type="button" disabled={disabled || saveDisabled} onClick={() => void onSave()}>
            {copy("Save & restart", "保存并重启", "儲存並重新啟動", "保存して再起動")}
          </button>
        ) : null}
      </div>
      {error ? <p className="harness-model-mapping-error" role="alert">{error}</p> : null}
      <div className="harness-model-mapping-grid" role="table">
        <div className="harness-model-mapping-columns" role="row">
          <span role="columnheader">{copy("Agent request", "Agent 请求", "Agent 請求", "Agent リクエスト")}</span>
          <span role="columnheader">{copy("Provider", "供应商", "供應商", "プロバイダー")}</span>
          <span role="columnheader">{copy("Model", "模型", "模型", "モデル")}</span>
        </div>
        {rows.map((row) => {
          const target = routes?.[row.requestedModel] ?? { upstream: null, model: null };
          const provider = providers.find((candidate) => candidate.name === target.upstream);
          const providerMissing = Boolean(target.upstream) && !provider;
          const selectedProviderOption: CompactComboboxOption[] = target.upstream
            ? [{
                value: target.upstream,
                label: providerMissing ? `${target.upstream}${staleSuffix}` : target.upstream,
                icon: <ProviderIcon
                  id={provider?.brand_id}
                  label={target.upstream}
                  size={20}
                />,
                hint: providerMissing
                  ? copy("Deleted", "已删除", "已刪除", "削除済み")
                  : provider?.access_tier === "free"
                    ? copy("Free", "免费", "免費", "無料")
                    : undefined,
              }]
            : [];
          const providerOptions: CompactComboboxOption[] = [
            ...selectedProviderOption,
            { value: "", label: notSelected },
            ...providers
              .filter((candidate) => candidate.name !== target.upstream)
              .map((candidate) => ({
                value: candidate.name,
                label: candidate.name,
                icon: <ProviderIcon id={candidate.brand_id} label={candidate.name} size={20} />,
                hint: candidate.access_tier === "free"
                  ? copy("Free", "免费", "免費", "無料")
                  : undefined,
              })),
          ];
          const providerModels = provider?.models ?? [];
          const modelMissing = Boolean(target.model) && !providerModels.includes(target.model as string);
          const modelOptions: CompactComboboxOption[] = [
            ...(modelMissing
              ? [{
                  value: target.model as string,
                  label: `${target.model}${staleSuffix}`,
                  hint: copy("Removed", "已下架", "已移除", "削除済み"),
                }]
              : []),
            { value: "", label: notSelected },
            ...providerModels.map((model) => ({ value: model, label: model })),
          ];

          return (
            <div className="harness-model-mapping-row" role="row" key={row.requestedModel}>
              <div className="harness-model-request" role="cell">
                <strong>{row.label}</strong>
                <code>{row.requestedModel}</code>
              </div>
              <div className="harness-model-choice harness-model-provider" role="cell">
                <span className="harness-model-mobile-label" aria-hidden="true">
                  {copy("Provider", "供应商", "供應商", "プロバイダー")}
                </span>
                <CompactCombobox
                  ariaLabel={copy(
                    `${row.label} provider`,
                    `${row.label} 供应商`,
                    `${row.label} 供應商`,
                    `${row.label} プロバイダー`,
                  )}
                  disabled={controlsDisabled}
                  value={target.upstream ?? ""}
                  options={providerOptions}
                  onChange={(upstream) => {
                    if (!upstream) {
                      void onChange?.(row.requestedModel, { upstream: null, model: null });
                      return;
                    }
                    const nextProvider = providers.find((candidate) => candidate.name === upstream);
                    void onChange?.(row.requestedModel, {
                      upstream,
                      model: nextProvider?.models[0] ?? null,
                    });
                  }}
                />
              </div>
              <div className="harness-model-choice harness-model-target" role="cell">
                <span className="harness-model-mobile-label" aria-hidden="true">
                  {copy("Model", "模型", "模型", "モデル")}
                </span>
                <CompactCombobox
                  ariaLabel={copy(
                    `${row.label} model`,
                    `${row.label} 模型`,
                    `${row.label} 模型`,
                    `${row.label} モデル`,
                  )}
                  disabled={controlsDisabled || !target.upstream}
                  value={target.model ?? ""}
                  options={modelOptions}
                  onChange={(model) => {
                    void onChange?.(row.requestedModel, {
                      upstream: target.upstream,
                      model: model || null,
                    });
                  }}
                />
              </div>
            </div>
          );
        })}
      </div>
    </section>
  );
}
