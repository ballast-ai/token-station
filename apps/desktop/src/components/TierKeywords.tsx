import { useState } from "react";
import type { TierSlot } from "../api";
import { useLocalizedCopy } from "./LanguageProvider";

/** Static copy for the three-tier keyword library. Order matches core rule priority
 *  from strong to medium to weak, so a phrase matching two tiers moves upward.
 *  Lists start empty and show only user-added keywords. */

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
      label: copy("High model", "强模型", "強模型", "強モデル"),
      hint: copy("Matches go directly to the high tier", "命中这些词 → 直接上最强档", "命中這些詞 → 直接上最強檔", "命中これらのキーワード → スマートな上位モデルに直接"),
      placeholder: copy("Enter a keyword and press Return", "输入关键词，回车加入", "輸入一個關鍵詞並按回車", "キーワードを入力し、Returnキーを押してください"),
    },
    {
      slot: "mid",
      label: copy("Medium model", "中模型", "中模型", "中モデル"),
      hint: copy("Matches go to the medium tier", "命中这些词 → 走中档", "命中這些詞 → 走中檔", "命中これらのキーワード → 中位モデルに移動"),
      placeholder: copy("Enter a keyword and press Return", "输入关键词，回车加入", "輸入一個關鍵詞並按回車", "キーワードを入力し、Returnキーを押してください"),
    },
    {
      slot: "low",
      label: copy("Low model", "弱模型", "弱模型", "弱モデル"),
      hint: copy("Matches go to the fast, low-cost tier", "命中这些词 → 走便宜快档", "命中這些詞 → 走便宜快檔", "命中これらのキーワード → 割安で速いモデルに移動"),
      placeholder: copy("Enter a keyword and press Return", "输入关键词，回车加入", "輸入一個關鍵詞並按回車", "キーワードを入力し、Returnキーを押してください"),
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
                aria-label={copy(`${label} keywords`, `${label}关键词列表`, `${label}關鍵詞列表`, `${label}キーワードリスト`)}
              >
                {words.map((word) => (
                  <span className="keyword-chip" key={word} role="listitem">
                    <span className="keyword-text">{word}</span>
                    <button
                      type="button"
                      className="keyword-remove"
                      aria-label={copy(`Delete keyword ${word}`, `删除关键词 ${word}`, `刪除關鍵詞 ${word}`, `キーワード ${word} を削除`)}
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
                  aria-label={copy(`${label} keyword`, `${label}关键词`, `${label}關鍵詞`, `${label}キーワード`)}
                  placeholder={ready ? placeholder : copy("This tier has no model", "该档未配置模型", "該檔未配置模型", "この階層にはモデルが設定されていません")}
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
                  {copy("Add", "添加", "新增", "追加")}
                </button>
              </div>
            </div>
          </div>
        );
      })}
    </div>
  );
}
