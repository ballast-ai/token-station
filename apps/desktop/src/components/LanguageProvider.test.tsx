import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it } from "vitest";
import {
  LANGUAGE_STORAGE_KEY,
  LanguageProvider,
  useLanguage,
} from "./LanguageProvider";

function LanguageProbe() {
  const { language, setLanguage, t, copy } = useLanguage();
  return (
    <div>
      <output>{`${language}:${t("settings.title")}:${copy("Home routing", "主页路由")}`}</output>
      <button type="button" onClick={() => setLanguage("en")}>English</button>
      <button type="button" onClick={() => setLanguage("zh-CN")}>简体中文</button>
    </div>
  );
}

beforeEach(() => {
  window.localStorage.clear();
  document.documentElement.lang = "";
});

describe("LanguageProvider", () => {
  it("defaults to English and synchronizes the document language", () => {
    render(
      <LanguageProvider>
        <LanguageProbe />
      </LanguageProvider>,
    );

    expect(screen.getByText("en:Settings:Home routing")).toBeInTheDocument();
    expect(document.documentElement).toHaveAttribute("lang", "en");
    expect(window.localStorage.getItem(LANGUAGE_STORAGE_KEY)).toBe("en");
  });

  it("restores and persists an explicit language selection", async () => {
    window.localStorage.setItem(LANGUAGE_STORAGE_KEY, "zh-CN");
    const user = userEvent.setup();
    render(
      <LanguageProvider>
        <LanguageProbe />
      </LanguageProvider>,
    );

    expect(screen.getByText("zh-CN:设置:主页路由")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "English" }));

    expect(screen.getByText("en:Settings:Home routing")).toBeInTheDocument();
    expect(document.documentElement).toHaveAttribute("lang", "en");
    expect(window.localStorage.getItem(LANGUAGE_STORAGE_KEY)).toBe("en");
  });

  it("migrates the cc-Switch-style zh value and falls back to English for unknown values", () => {
    window.localStorage.setItem(LANGUAGE_STORAGE_KEY, "zh");
    const { unmount } = render(
      <LanguageProvider>
        <LanguageProbe />
      </LanguageProvider>,
    );
    expect(screen.getByText("zh-CN:设置:主页路由")).toBeInTheDocument();
    unmount();

    window.localStorage.setItem(LANGUAGE_STORAGE_KEY, "klingon");
    render(
      <LanguageProvider>
        <LanguageProbe />
      </LanguageProvider>,
    );
    expect(screen.getByText("en:Settings:Home routing")).toBeInTheDocument();
  });

  it("rejects useLanguage outside its provider", () => {
    expect(() => render(<LanguageProbe />)).toThrow(
      "useLanguage must be used within LanguageProvider",
    );
  });
});
