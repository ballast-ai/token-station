import { useState } from "react";
import type { TierSlot } from "../api";
import { useLocalizedCopy } from "./LanguageProvider";

/** 三档关键词库的静态文案。顺序=优先级(强→中→弱),与内核 rules 定序一致:
 *  同一句话同时命中两档的词时向上升档。列表只显示用户自己加的词,从空开始。 */

export interface TierKeywordsProps {
  keywords: Record<TierSlot, string[]>;
  disabled?: boolean;
  /** 该档是否已配置好供应商+模型(未配置则不能加词,内核会拒绝空池规则)。 */
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
  // 每档一个独立输入框状态。
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
                      ? copy(
                          "No keywords yet. Add one to keep matching requests in this tier.",
                          "这一档还没有关键词。添加关键词后，匹配的请求会固定到这一档。",
                        )
                      : copy(
                          "Select a provider and model for this tier first.",
                          "先在上方为该档选好供应商和模型。",
                        )}
                  </span>
                )}
                {words.map((word) => (
                  <span className="keyword-chip" key={word}>
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
