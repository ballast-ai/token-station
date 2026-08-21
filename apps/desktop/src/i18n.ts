export type Language = "zh-CN" | "zh-TW" | "en" | "ja";

let activeLanguage: Language | null = null;

export function setActiveLanguage(language: Language): void {
  activeLanguage = language;
}

export function getActiveLanguage(): Language | null {
  return activeLanguage;
}

export function localizedCopy(
  language: Language,
  english: string,
  simplifiedChinese: string,
  traditionalChinese: string,
  japanese: string,
): string {
  return {
    en: english,
    "zh-CN": simplifiedChinese,
    "zh-TW": traditionalChinese,
    ja: japanese,
  }[language];
}
