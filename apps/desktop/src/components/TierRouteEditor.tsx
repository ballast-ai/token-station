import type { ProviderView, TierSlot, TierView } from "../api";

const TIER_META: { slot: TierSlot; label: string; hint: string }[] = [
  { slot: "high", label: "上档", hint: "最强模型 · 难任务升到这里" },
  { slot: "mid", label: "中档", hint: "中等复杂度" },
  { slot: "low", label: "下档", hint: "便宜快模型 · 简单任务兜底" },
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
      <div className="tier-col-head" />
      <div className="tier-col-head">供应商</div>
      <div className="tier-col-head">模型</div>

      {TIER_META.map(({ slot, label, hint }) => {
        const tier = tiers[slot];
        const provider = providers.find((candidate) => candidate.name === tier.upstream);

        return (
          <div className="tier-row" key={slot}>
            <div className={`tier-badge ${slot}`}>
              <div className="tier-label">{label}</div>
              <div className="tier-hint">{hint}</div>
            </div>
            <select
              className="select"
              aria-label={`${label}供应商`}
              aria-readonly={readOnly}
              disabled={controlsDisabled}
              value={tier.upstream ?? ""}
              onChange={(event) => {
                const upstream = event.target.value;
                if (!upstream) {
                  void onTierChange(slot, null, null);
                  return;
                }

                const nextProvider = providers.find((candidate) => candidate.name === upstream);
                void onTierChange(slot, upstream, nextProvider?.models[0] ?? null);
              }}
            >
              <option value="">— 未选 —</option>
              {providers.map((candidate) => (
                <option key={candidate.name} value={candidate.name}>
                  {candidate.name}{candidate.access_tier === "free" ? " · 免费" : ""}
                </option>
              ))}
            </select>
            <select
              className="select"
              aria-label={`${label}模型`}
              aria-readonly={readOnly}
              value={tier.model ?? ""}
              disabled={controlsDisabled || !tier.upstream}
              onChange={(event) => {
                const model = event.target.value;
                void onTierChange(slot, tier.upstream, model || null);
              }}
            >
              <option value="">— 模型 —</option>
              {provider?.models.map((model) => (
                <option key={model} value={model}>
                  {model}
                </option>
              ))}
            </select>
          </div>
        );
      })}
    </div>
  );
}
