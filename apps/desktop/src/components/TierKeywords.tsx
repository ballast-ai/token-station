import { useEffect, useState } from "react";
import type { TierSlot } from "../api";
import { useLocalizedCopy } from "./LanguageProvider";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "./ui/dialog";

export interface TierKeywordsProps {
  keywords: Record<TierSlot, string[]>;
  disabled?: boolean;
  configured: Record<TierSlot, boolean>;
  activeSlot: TierSlot | null;
  onOpenChange: (open: boolean) => void;
  onAdd: (slot: TierSlot, keyword: string) => void | Promise<void>;
  onRemove: (slot: TierSlot, keyword: string) => void | Promise<void>;
}

export default function TierKeywords({
  keywords,
  disabled = false,
  configured,
  activeSlot,
  onOpenChange,
  onAdd,
  onRemove,
}: TierKeywordsProps) {
  const { copy } = useLocalizedCopy();
  const [draft, setDraft] = useState("");
  useEffect(() => setDraft(""), [activeSlot]);

  const tierMeta: Record<TierSlot, { label: string; hint: string }> = {
    high: {
      label: copy("High tier", "上档", "上檔", "上位"),
      hint: copy(
        "Matches stay on the high tier and override automatic classification.",
        "命中后固定走上档，优先于自动判断。",
        "命中後固定走上檔，優先於自動判斷。",
        "一致すると上位に固定され、自動分類より優先されます。",
      ),
    },
    mid: {
      label: copy("Medium tier", "中档", "中檔", "中位"),
      hint: copy(
        "Matches stay on the medium tier and override automatic classification.",
        "命中后固定走中档，优先于自动判断。",
        "命中後固定走中檔，優先於自動判斷。",
        "一致すると中位に固定され、自動分類より優先されます。",
      ),
    },
    low: {
      label: copy("Low tier", "下档", "下檔", "下位"),
      hint: copy(
        "Matches stay on the low tier and override automatic classification.",
        "命中后固定走下档，优先于自动判断。",
        "命中後固定走下檔，優先於自動判斷。",
        "一致すると下位に固定され、自動分類より優先されます。",
      ),
    },
  };

  const submit = () => {
    if (!activeSlot) return;
    const word = draft.trim();
    if (!word) return;
    void Promise.resolve(onAdd(activeSlot, word)).then(() => setDraft(""));
  };

  if (!activeSlot) return null;
  const meta = tierMeta[activeSlot];
  const words = keywords[activeSlot] ?? [];
  const ready = configured[activeSlot];
  const rowDisabled = disabled || !ready;

  return (
    <Dialog open onOpenChange={onOpenChange}>
      <DialogContent className="tier-keyword-dialog" closeLabel={copy("Close", "关闭", "關閉", "閉じる")}>
        <DialogHeader>
          <DialogTitle>{copy(`${meta.label} keywords`, `${meta.label}关键词`, `${meta.label}關鍵詞`, `${meta.label}キーワード`)}</DialogTitle>
          <DialogDescription>{meta.hint}</DialogDescription>
        </DialogHeader>

        <div className="tier-keyword-editor">
          <div
            className="keyword-chips"
            role="list"
            aria-label={copy(`${meta.label} keyword list`, `${meta.label}关键词列表`, `${meta.label}關鍵詞列表`, `${meta.label}キーワードリスト`)}
          >
            {words.map((word) => (
              <span className="keyword-chip" key={word} role="listitem">
                <span className="keyword-text">{word}</span>
                <button
                  type="button"
                  className="keyword-remove"
                  aria-label={copy(`Delete keyword ${word}`, `删除关键词 ${word}`, `刪除關鍵詞 ${word}`, `キーワード ${word} を削除`)}
                  disabled={disabled}
                  onClick={() => void onRemove(activeSlot, word)}
                >
                  ×
                </button>
              </span>
            ))}
            {words.length === 0 && (
              <span className="tier-keyword-empty">{copy("No keywords yet", "还没有关键词", "尚無關鍵詞", "キーワードはまだありません")}</span>
            )}
          </div>

          <div className="keyword-add">
            <input
              className="input"
              aria-label={copy(`${meta.label} keyword`, `${meta.label}关键词`, `${meta.label}關鍵詞`, `${meta.label}キーワード`)}
              placeholder={ready
                ? copy("Enter a keyword and press Return", "输入关键词，回车加入", "輸入關鍵詞並按 Enter", "キーワードを入力して Return")
                : copy("This tier has no model", "该档未配置模型", "該檔未配置模型", "この階層にはモデルがありません")}
              value={draft}
              disabled={rowDisabled}
              onChange={(event) => setDraft(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter") {
                  event.preventDefault();
                  submit();
                }
              }}
            />
            <button type="button" className="btn primary" disabled={rowDisabled || !draft.trim()} onClick={submit}>
              {copy("Add", "添加", "新增", "追加")}
            </button>
          </div>
          {!ready && <p className="tier-keyword-unavailable">{copy(
            "Select a provider and model for this tier first.",
            "请先为该档选择供应商和模型。",
            "請先為該檔選擇供應商和模型。",
            "先にこの階層のプロバイダーとモデルを選択してください。",
          )}</p>}
        </div>

        <DialogFooter>
          <button type="button" className="btn" onClick={() => onOpenChange(false)}>
            {copy("Done", "完成", "完成", "完了")}
          </button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
