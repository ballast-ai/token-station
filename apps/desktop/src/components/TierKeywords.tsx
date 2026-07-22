import { useState } from "react";
import type { TierSlot } from "../api";

/** Static text for the three-tier keyword library. Order sets priority from strong to medium to weak and matches core rule order:
 * If one sentence matches terms in two tiers, use the higher tier. The list starts empty and shows only user-added terms. */
const TIER_META: {
  slot: TierSlot;
  label: string;
  hint: string;
  placeholder: string;
}[] = [
  { slot: "high", label: "强模型", hint: "命中这些词 → 直接上最强档", placeholder: "输入关键词,回车加入" },
  { slot: "mid", label: "中模型", hint: "命中这些词 → 走中档", placeholder: "输入关键词,回车加入" },
  { slot: "low", label: "弱模型", hint: "命中这些词 → 走便宜快档", placeholder: "输入关键词,回车加入" },
];

export interface TierKeywordsProps {
  keywords: Record<TierSlot, string[]>;
  disabled?: boolean;
  /** Whether this tier has a provider and model; the core rejects keywords targeting an empty pool. */
  configured: Record<TierSlot, boolean>;
  onAdd: (slot: TierSlot, keyword: string) => void | Promise<void>;
  onRemove: (slot: TierSlot, keyword: string) => void | Promise<void>;
}

export default function TierKeywords({
  keywords,
  disabled = false,
  configured,
  onAdd,
  onRemove,
}: TierKeywordsProps) {
  // Keep independent input state for each tier.
  const [drafts, setDrafts] = useState<Record<TierSlot, string>>({
    high: "",
    mid: "",
    low: "",
  });

  const setDraft = (slot: TierSlot, value: string) =>
    setDrafts((current) => ({ ...current, [slot]: value }));

  const submit = (slot: TierSlot) => {
    const word = drafts[slot].trim();
    if (!word) return;
    void Promise.resolve(onAdd(slot, word)).then(() => setDraft(slot, ""));
  };

  return (
    <div className="keyword-grid">
      {TIER_META.map(({ slot, label, hint, placeholder }) => {
        const words = keywords[slot] ?? [];
        const ready = configured[slot];
        const rowDisabled = disabled || !ready;

        return (
          <div className="keyword-row" key={slot}>
            <div className={`tier-badge ${slot}`}>
              <div className="tier-label">{label}</div>
              <div className="tier-hint">{hint}</div>
            </div>

            <div className="keyword-body">
              <div className="keyword-chips">
                {words.length === 0 && (
                  <span className="keyword-empty">
                    {ready
                      ? "这一档还没有关键词 —— 加一个,含它的请求就归你管、走这一档"
                      : "先在上方为该档选好供应商和模型"}
                  </span>
                )}
                {words.map((word) => (
                  <span className="keyword-chip" key={word}>
                    <span className="keyword-text">{word}</span>
                    <button
                      type="button"
                      className="keyword-remove"
                      aria-label={`删除关键词 ${word}`}
                      disabled={disabled}
                      onClick={() => void onRemove(slot, word)}
                    >
                      ×
                    </button>
                  </span>
                ))}
              </div>

              <div className="keyword-add">
                <input
                  className="input"
                  placeholder={ready ? placeholder : "该档未配置模型"}
                  value={drafts[slot]}
                  disabled={rowDisabled}
                  onChange={(event) => setDraft(slot, event.target.value)}
                  onKeyDown={(event) => {
                    if (event.key === "Enter") {
                      event.preventDefault();
                      submit(slot);
                    }
                  }}
                />
                <button
                  type="button"
                  className="btn"
                  disabled={rowDisabled || !drafts[slot].trim()}
                  onClick={() => submit(slot)}
                >
                  添加
                </button>
              </div>
            </div>
          </div>
        );
      })}
    </div>
  );
}
