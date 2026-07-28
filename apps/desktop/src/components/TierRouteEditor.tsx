import type { ProviderView, TierSlot, TierView } from "../api";
import CompactCombobox, { type CompactComboboxOption } from "./CompactCombobox";

const TIER_META: { slot: TierSlot; label: string; hint: string }[] = [
  { slot: "high", label: "上档", hint: "复杂推理与代码" },
  { slot: "mid", label: "中档", hint: "日常开发任务" },
  { slot: "low", label: "下档", hint: "简单快速任务" },
];

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

  return (
    <div className="tier-grid">
      <div className="tier-table-head">
        <div className="tier-col-head">档位</div>
        <div className="tier-col-head">供应商</div>
        <div className="tier-col-head">模型</div>
      </div>

      {TIER_META.map(({ slot, label, hint }) => {
        const tier = tiers[slot];
        const provider = providers.find((candidate) => candidate.name === tier.upstream);
        const selectedProviderOption: CompactComboboxOption[] = tier.upstream
          ? [{
              value: tier.upstream,
              label: tier.upstream,
              hint: provider?.access_tier === "free" ? "免费" : undefined,
            }]
          : [];
        const providerOptions: CompactComboboxOption[] = [
          ...selectedProviderOption,
          { value: "", label: "未选择" },
          ...providers
            .filter((candidate) => candidate.name !== tier.upstream)
            .map((candidate) => ({
              value: candidate.name,
              label: candidate.name,
              hint: candidate.access_tier === "free" ? "免费" : undefined,
            })),
        ];
        const modelOptions: CompactComboboxOption[] = [
          { value: "", label: "未选择" },
          ...(provider?.models ?? []).map((model) => ({ value: model, label: model })),
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
              ariaLabel={`${label}供应商`}
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
              ariaLabel={`${label}模型`}
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
