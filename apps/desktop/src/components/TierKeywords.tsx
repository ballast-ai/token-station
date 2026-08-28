import { useId, useState } from "react";
import type { TierSlot } from "../api";
import { useLocalizedCopy } from "./LanguageProvider";
import { Button } from "./ui/button";
import { Input } from "./ui/input";

export interface TierKeywordsProps {
  slot: TierSlot;
  keywords: string[];
  disabled?: boolean;
  configured: boolean;
  onAdd: (slot: TierSlot, keyword: string) => boolean | void | Promise<boolean | void>;
  onRemove: (slot: TierSlot, keyword: string) => void | Promise<void>;
}

export default function TierKeywords({
  slot,
  keywords,
  disabled = false,
  configured,
  onAdd,
  onRemove,
}: TierKeywordsProps) {
  const { copy } = useLocalizedCopy();
  const [draft, setDraft] = useState("");
  const [validationError, setValidationError] = useState<string | null>(null);
  const errorId = useId();
  const label = {
    high: copy("High tier", "上档", "上檔", "上位"),
    mid: copy("Medium tier", "中档", "中檔", "中位"),
    low: copy("Low tier", "下档", "下檔", "下位"),
  }[slot];
  const accessibleLabel = copy(
    `${label} keywords`,
    `${label}关键词`,
    `${label}關鍵詞`,
    `${label}キーワード`,
  );
  const inputDisabled = disabled || !configured;

  const submit = () => {
    const keyword = draft.trim();
    if (!keyword || inputDisabled) return;
    if (keywords.some((existing) => existing.toLowerCase() === keyword.toLowerCase())) {
      setValidationError(copy(
        `Keyword “${keyword}” already exists`,
        `关键词“${keyword}”已存在`,
        `關鍵詞「${keyword}」已存在`,
        `キーワード「${keyword}」は既に存在します`,
      ));
      return;
    }
    setValidationError(null);
    void Promise.resolve(onAdd(slot, keyword)).then((added) => {
      if (added !== false) setDraft("");
    });
  };

  return (
    <div className="tier-keyword-row" role="group" aria-label={accessibleLabel}>
      <span className="tier-keyword-label">{copy("Keywords", "关键词", "關鍵詞", "キーワード")}</span>
      <div
        className="tier-keyword-values"
        role="list"
        data-presentation="plain-text"
        aria-label={copy(
          `${label} configured keywords`,
          `${label}已设置关键词`,
          `${label}已設定關鍵詞`,
          `${label}の設定済みキーワード`,
        )}
      >
        {keywords.map((keyword) => (
          <span className="tier-keyword-value" key={keyword} role="listitem">
            <span>{keyword}</span>
            <Button
              className="tier-keyword-remove"
              variant="ghost"
              size="icon-xs"
              type="button"
              disabled={disabled}
              aria-label={copy(
                `Delete keyword ${keyword}`,
                `删除关键词 ${keyword}`,
                `刪除關鍵詞 ${keyword}`,
                `キーワード ${keyword} を削除`,
              )}
              onClick={() => void onRemove(slot, keyword)}
            >
              ×
            </Button>
          </span>
        ))}
      </div>
      <Input
        className="tier-keyword-input"
        aria-label={accessibleLabel}
        placeholder={configured
          ? copy(
              "Enter a keyword, then press Return",
              "输入关键词后按回车",
              "輸入關鍵詞後按 Enter",
              "キーワードを入力して Return",
            )
          : copy(
              "Select a provider and model first",
              "请先选择供应商和模型",
              "請先選擇供應商和模型",
              "先にプロバイダーとモデルを選択",
            )}
        value={draft}
        disabled={inputDisabled}
        aria-invalid={validationError ? true : undefined}
        aria-describedby={validationError ? errorId : undefined}
        onChange={(event) => {
          setDraft(event.target.value);
          setValidationError(null);
        }}
        onKeyDown={(event) => {
          if (event.key === "Enter") {
            event.preventDefault();
            submit();
          }
        }}
      />
      {validationError && (
        <span className="tier-keyword-error" id={errorId} role="alert">
          {validationError}
        </span>
      )}
    </div>
  );
}
