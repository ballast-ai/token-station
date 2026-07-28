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
  onSyncTiers?: () => void | Promise<void>;
}

export default function TierRouteEditor({
  tiers,
  providers,
  disabled = false,
  readOnly = false,
  onTierChange,
  onSyncTiers,
}: TierRouteEditorProps) {
  const controlsDisabled = disabled || readOnly;
  const providerOptions: CompactComboboxOption[] = [
    { value: "", label: "未选择" },
    ...providers.map((provider) => ({
      value: provider.name,
      label: provider.name,
      hint: provider.access_tier === "free" ? "免费" : undefined,
    })),
  ];
  const canSync = Boolean(tiers.high.upstream && tiers.high.model);

  return (
    <div className={`tier-grid ${onSyncTiers ? "with-sync" : ""}`}>
      <div className="tier-table-head">
        <div className="tier-col-head">档位</div>
        <div className="tier-col-head">供应商</div>
        <div className="tier-col-head">模型</div>
        {onSyncTiers && <div className="tier-col-head" />}
      </div>

      {TIER_META.map(({ slot, label, hint }, index) => {
        const tier = tiers[slot];
        const provider = providers.find((candidate) => candidate.name === tier.upstream);
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
            {onSyncTiers && (
              <div className="tier-row-action">
                {index === 0 && (
                  <button
                    className="btn quiet sync-tiers-action"
                    type="button"
                    disabled={controlsDisabled || !canSync}
                    onClick={() => void onSyncTiers()}
                  >
                    同步三档
                  </button>
                )}
              </div>
            )}
          </div>
        );
      })}
    </div>
  );
}
