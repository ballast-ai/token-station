import type { ProviderView, TierSlot, TierView } from "../api";
import { ProviderIcon } from "../brandIcons";
import CompactCombobox, { type CompactComboboxOption } from "./CompactCombobox";
import { useLocalizedCopy } from "./LanguageProvider";
import TierKeywords from "./TierKeywords";

export interface TierRouteEditorProps {
  tiers: Record<TierSlot, TierView>;
  providers: ProviderView[];
  disabled?: boolean;
  readOnly?: boolean;
  keywords?: Record<TierSlot, string[]>;
  onAddKeyword?: (slot: TierSlot, keyword: string) => boolean | void | Promise<boolean | void>;
  onRemoveKeyword?: (slot: TierSlot, keyword: string) => void | Promise<void>;
  onTierChange: (
    slot: TierSlot,
    upstream: string | null,
    model: string | null,
  ) => void | Promise<void>;
}

export default function TierRouteEditor({
  tiers,
  providers,
  disabled = false,
  readOnly = false,
  keywords,
  onAddKeyword,
  onRemoveKeyword,
  onTierChange,
}: TierRouteEditorProps) {
  const controlsDisabled = disabled || readOnly;
  const { copy } = useLocalizedCopy();
  const tierMeta: { slot: TierSlot; label: string }[] = [
    { slot: "high", label: copy("High", "上档", "上檔", "上位モデル") },
    { slot: "mid", label: copy("Medium", "中档", "中檔", "中位") },
    { slot: "low", label: copy("Low", "下档", "下檔", "下位") },
  ];

  return (
    <div className={`tier-grid${onAddKeyword ? " tier-grid-with-keywords" : ""}`}>
      <div className="tier-table-head">
        <div className="tier-col-head">{copy("Tier", "档位", "檔位", "グレード")}</div>
        <div className="tier-col-head">{copy("Provider", "供应商", "供應商", "プロバイダー")}</div>
        <div className="tier-col-head">{copy("Model", "模型", "模型", "モデル")}</div>
      </div>

      {tierMeta.map(({ slot, label }) => {
        const tier = tiers[slot];
        const provider = providers.find((candidate) => candidate.name === tier.upstream);
        // A stored selection whose provider/model no longer exists in the shared
        // pool is shown as invalid — not silently re-listed as if valid — so the
        // user notices and reselects, avoiding the stale-option residue they reported. The valid
        // choices stay in the dropdown, so reselecting is one click away.
        const staleSuffix = copy(" (unavailable — reselect)", "（已失效·请重选）", "（已失效·請重選）", "（無効·再度選択）");
        const providerMissing = Boolean(tier.upstream) && !provider;
        const selectedProviderOption: CompactComboboxOption[] = tier.upstream
          ? [{
              value: tier.upstream,
              label: providerMissing ? `${tier.upstream}${staleSuffix}` : tier.upstream,
              icon: <ProviderIcon
                id={provider?.brand_id}
                label={tier.upstream}
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
          { value: "", label: copy("Not selected", "未选择", "未選擇", "選択されていません") },
          ...providers
            .filter((candidate) => candidate.name !== tier.upstream)
            .map((candidate) => ({
              value: candidate.name,
              label: candidate.name,
              icon: <ProviderIcon id={candidate.brand_id} label={candidate.name} size={20} />,
              hint: candidate.access_tier === "free" ? copy("Free", "免费", "免費", "無料") : undefined,
            })),
        ];
        const providerModels = provider?.models ?? [];
        const modelMissing =
          Boolean(tier.model) && !providerModels.includes(tier.model as string);
        const selectedModelOption: CompactComboboxOption[] = modelMissing
          ? [{
              value: tier.model as string,
              label: `${tier.model}${staleSuffix}`,
              hint: copy("Removed", "已下架", "已移除", "削除済み"),
            }]
          : [];
        const modelOptions: CompactComboboxOption[] = [
          ...selectedModelOption,
          { value: "", label: copy("Not selected", "未选择", "未選擇", "選択されていません") },
          ...providerModels.map((model) => ({ value: model, label: model })),
        ];
        const tierKeywords = keywords?.[slot] ?? [];

        return (
          <div className={`tier-row tier-row-${slot}`} key={slot}>
            <div className="tier-identity">
              <span className={`tier-node ${slot}`} aria-hidden="true" />
              <div className="tier-copy">
                <strong>{label}</strong>
              </div>
            </div>
            <CompactCombobox
              ariaLabel={copy(`${label} provider`, `${label}供应商`, `${label} 供應商`, `${label} プロバイダー`)}
              disabled={controlsDisabled}
              value={tier.upstream ?? ""}
              options={providerOptions}
              onChange={(upstream) => {
                if (!upstream) {
                  void onTierChange(slot, null, null);
                  return;
                }

                const nextProvider = providers.find((candidate) => candidate.name === upstream);
                void onTierChange(slot, upstream, nextProvider?.models[0] ?? null);
              }}
            />
            <CompactCombobox
              ariaLabel={copy(`${label} model`, `${label}模型`, `${label} 模型`, `${label} モデル`)}
              value={tier.model ?? ""}
              disabled={controlsDisabled || !tier.upstream}
              options={modelOptions}
              onChange={(model) => {
                void onTierChange(slot, tier.upstream, model || null);
              }}
            />
            {onAddKeyword && onRemoveKeyword && (
              <TierKeywords
                slot={slot}
                keywords={tierKeywords}
                configured={Boolean(tier.upstream && tier.model)}
                disabled={controlsDisabled}
                onAdd={onAddKeyword}
                onRemove={onRemoveKeyword}
              />
            )}
          </div>
        );
      })}
    </div>
  );
}
