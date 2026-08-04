import { useState } from "react";
import type { TierSlot } from "../api";
import { useLocalizedCopy } from "./LanguageProvider";

/** Static text for the three-tier keyword library. Order sets priority from strong to medium to weak and matches core rule order:
 * If one sentence matches terms in two tiers, use the higher tier. The list starts empty and shows only user-added terms. */

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
  const { copy } = useLocalizedCopy();
  const tierMeta: {
    slot: TierSlot;
    label: string;
    hint: string;
    placeholder: string;
  }[] = [
    {
      slot: "high",
      label: copy("High model", "强模型"),
      hint: copy("Matches go directly to the high tier", "命中这些词 → 直接上最强档"),
      placeholder: copy("Enter a keyword and press Return", "输入关键词，回车加入"),
    },
    {
      slot: "mid",
      label: copy("Medium model", "中模型"),
      hint: copy("Matches go to the medium tier", "命中这些词 → 走中档"),
      placeholder: copy("Enter a keyword and press Return", "输入关键词，回车加入"),
    },
    {
      slot: "low",
      label: copy("Low model", "弱模型"),
      hint: copy("Matches go to the fast, low-cost tier", "命中这些词 → 走便宜快档"),
      placeholder: copy("Enter a keyword and press Return", "输入关键词，回车加入"),
    },
  ];
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
      {tierMeta.map(({ slot, label, hint, placeholder }) => {
        const words = keywords[slot] ?? [];
        const ready = configured[slot];
        const rowDisabled = disabled || !ready;

        return (
          <div className="keyword-row" key={slot} role="group" aria-label={label}>
            <div className={`tier-badge ${slot}`}>
              <div className="tier-label">{label}</div>
              <div className="tier-hint">{hint}</div>
            </div>

            <div className="keyword-body">
              <div
                className="keyword-chips"
                role="list"
                aria-label={copy(`${label} keywords`, `${label}关键词列表`)}
              >
                {words.map((word) => (
                  <span className="keyword-chip" key={word} role="listitem">
                    <span className="keyword-text">{word}</span>
                    <button
                      type="button"
                      className="keyword-remove"
                      aria-label={copy(`Delete keyword ${word}`, `删除关键词 ${word}`)}
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
                  aria-label={copy(`${label} keyword`, `${label}关键词`)}
                  placeholder={ready ? placeholder : copy("This tier has no model", "该档未配置模型")}
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
                  {copy("Add", "添加")}
                </button>
              </div>
            </div>
          </div>
        );
      })}
    </div>
  );
}
