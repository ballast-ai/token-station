import type { ProviderView, TierSlot, TierView } from "../api";
import CompactCombobox, { type CompactComboboxOption } from "./CompactCombobox";
import { useLocalizedCopy } from "./LanguageProvider";

export interface TierRouteEditorProps {
  tiers: Record<TierSlot, TierView>;
  providers: ProviderView[];
  disabled?: boolean;
  readOnly?: boolean;
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
  onTierChange,
}: TierRouteEditorProps) {
  const controlsDisabled = disabled || readOnly;
  const { copy } = useLocalizedCopy();
  const tierMeta: { slot: TierSlot; label: string; hint: string }[] = [
    { slot: "high", label: copy("High", "上档"), hint: copy("Complex reasoning and code", "复杂推理与代码") },
    { slot: "mid", label: copy("Medium", "中档"), hint: copy("Everyday development", "日常开发任务") },
    { slot: "low", label: copy("Low", "下档"), hint: copy("Simple, fast tasks", "简单快速任务") },
  ];

  return (
    <div className="tier-grid">
      <div className="tier-table-head">
        <div className="tier-col-head">{copy("Tier", "档位")}</div>
        <div className="tier-col-head">{copy("Provider", "供应商")}</div>
        <div className="tier-col-head">{copy("Model", "模型")}</div>
      </div>

      {tierMeta.map(({ slot, label, hint }) => {
        const tier = tiers[slot];
        const provider = providers.find((candidate) => candidate.name === tier.upstream);
        // A stored selection whose provider/model no longer exists in the shared
        // pool is shown as invalid — not silently re-listed as if valid — so the
        // user notices and reselects, avoiding the stale-option residue they reported. The valid
        // choices stay in the dropdown, so reselecting is one click away.
        const staleSuffix = copy(" (unavailable — reselect)", "（已失效·请重选）");
        const providerMissing = Boolean(tier.upstream) && !provider;
        const selectedProviderOption: CompactComboboxOption[] = tier.upstream
          ? [{
              value: tier.upstream,
              label: providerMissing ? `${tier.upstream}${staleSuffix}` : tier.upstream,
              hint: providerMissing
                ? copy("Deleted", "已删除")
                : provider?.access_tier === "free"
                  ? copy("Free", "免费")
                  : undefined,
            }]
          : [];
        const providerOptions: CompactComboboxOption[] = [
          ...selectedProviderOption,
          { value: "", label: copy("Not selected", "未选择") },
          ...providers
            .filter((candidate) => candidate.name !== tier.upstream)
            .map((candidate) => ({
              value: candidate.name,
              label: candidate.name,
              hint: candidate.access_tier === "free" ? copy("Free", "免费") : undefined,
            })),
        ];
        const providerModels = provider?.models ?? [];
        const modelMissing =
          Boolean(tier.model) && !providerModels.includes(tier.model as string);
        const selectedModelOption: CompactComboboxOption[] = modelMissing
          ? [{
              value: tier.model as string,
              label: `${tier.model}${staleSuffix}`,
              hint: copy("Removed", "已下架"),
            }]
          : [];
        const modelOptions: CompactComboboxOption[] = [
          ...selectedModelOption,
          { value: "", label: copy("Not selected", "未选择") },
          ...providerModels.map((model) => ({ value: model, label: model })),
        ];

        return (
          <div className={`tier-row tier-row-${slot}`} key={slot}>
            <div className="tier-identity">
              <span className={`tier-node ${slot}`} aria-hidden="true" />
              <div className="tier-copy">
                <strong>{label}</strong>
                <span>{hint}</span>
              </div>
            </div>
            <CompactCombobox
              ariaLabel={copy(`${label} provider`, `${label}供应商`)}
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
              ariaLabel={copy(`${label} model`, `${label}模型`)}
              value={tier.model ?? ""}
              disabled={controlsDisabled || !tier.upstream}
              options={modelOptions}
              onChange={(model) => {
                void onTierChange(slot, tier.upstream, model || null);
              }}
            />
          </div>
        );
      })}
    </div>
  );
}
