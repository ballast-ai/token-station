import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it } from "vitest";
import {
  LANGUAGE_STORAGE_KEY,
  LanguageProvider,
  useLanguage,
} from "./LanguageProvider";

function LanguageProbe() {
  const { language, setLanguage, t } = useLanguage();
  return (
    <div>
      <output>{`${language}:${t("settings.title")}`}</output>
      <button type="button" onClick={() => setLanguage("en")}>English</button>
      <button type="button" onClick={() => setLanguage("ja")}>Japanese</button>
    </div>
  );
}

beforeEach(() => {
  window.localStorage.clear();
  document.documentElement.lang = "";
});

describe("LanguageProvider", () => {
  it("defaults to Simplified Chinese and synchronizes the document language", () => {
    render(
      <LanguageProvider>
        <LanguageProbe />
      </LanguageProvider>,
    );

    expect(screen.getByText("zh-CN:设置")).toBeInTheDocument();
    expect(document.documentElement).toHaveAttribute("lang", "zh-CN");
    expect(window.localStorage.getItem(LANGUAGE_STORAGE_KEY)).toBe("zh-CN");
  });

  it("restores and persists an explicit language selection", async () => {
    window.localStorage.setItem(LANGUAGE_STORAGE_KEY, "ja");
    const user = userEvent.setup();
    render(
      <LanguageProvider>
        <LanguageProbe />
      </LanguageProvider>,
    );

    expect(screen.getByText("ja:設定")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "English" }));

    expect(screen.getByText("en:Settings")).toBeInTheDocument();
    expect(document.documentElement).toHaveAttribute("lang", "en");
    expect(window.localStorage.getItem(LANGUAGE_STORAGE_KEY)).toBe("en");
  });

  it("migrates the cc-Switch-style zh value and ignores unknown values", () => {
    window.localStorage.setItem(LANGUAGE_STORAGE_KEY, "zh");
    const { unmount } = render(
      <LanguageProvider>
        <LanguageProbe />
      </LanguageProvider>,
    );
    expect(screen.getByText("zh-CN:设置")).toBeInTheDocument();
    unmount();

    window.localStorage.setItem(LANGUAGE_STORAGE_KEY, "klingon");
    render(
      <LanguageProvider>
        <LanguageProbe />
      </LanguageProvider>,
    );
    expect(screen.getByText("zh-CN:设置")).toBeInTheDocument();
  });

  it("rejects useLanguage outside its provider", () => {
    expect(() => render(<LanguageProbe />)).toThrow(
      "useLanguage must be used within LanguageProvider",
    );
  });
});
