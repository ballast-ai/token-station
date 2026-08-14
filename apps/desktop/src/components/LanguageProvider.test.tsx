import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  LANGUAGE_STORAGE_KEY,
  LanguageProvider,
  useLanguage,
} from "./LanguageProvider";
import { ErrorToastProvider } from "./ErrorToast";

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

function setBrowserLanguages(languages: string[]) {
  Object.defineProperty(window.navigator, "languages", {
    configurable: true,
    value: languages,
  });
  Object.defineProperty(window.navigator, "language", {
    configurable: true,
    value: languages[0] ?? "",
  });
}

beforeEach(() => {
  vi.restoreAllMocks();
  window.localStorage.clear();
  document.documentElement.lang = "";
  setBrowserLanguages(["en-US"]);
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

  it("detects Simplified Chinese from the browser language priority on first launch", () => {
    setBrowserLanguages(["ja-JP", "zh-Hans-CN", "en-US"]);

    render(
      <LanguageProvider>
        <LanguageProbe />
      </LanguageProvider>,
    );

    expect(screen.getByText("zh-CN:设置:主页路由")).toBeInTheDocument();
    expect(document.documentElement).toHaveAttribute("lang", "zh-CN");
    expect(window.localStorage.getItem(LANGUAGE_STORAGE_KEY)).toBe("zh-CN");
  });

  it("keeps an explicit saved language ahead of the browser language", () => {
    setBrowserLanguages(["zh-CN", "en-US"]);
    window.localStorage.setItem(LANGUAGE_STORAGE_KEY, "en");

    render(
      <LanguageProvider>
        <LanguageProbe />
      </LanguageProvider>,
    );

    expect(screen.getByText("en:Settings:Home routing")).toBeInTheDocument();
  });

  it("skips unsupported locales in order and falls back to English", () => {
    setBrowserLanguages(["ja-JP", "zh-TW", "en-GB"]);
    const { unmount } = render(
      <LanguageProvider>
        <LanguageProbe />
      </LanguageProvider>,
    );
    expect(screen.getByText("en:Settings:Home routing")).toBeInTheDocument();
    unmount();

    window.localStorage.clear();
    setBrowserLanguages(["ja-JP", "fr-FR"]);
    render(
      <LanguageProvider>
        <LanguageProbe />
      </LanguageProvider>,
    );
    expect(screen.getByText("en:Settings:Home routing")).toBeInTheDocument();
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

  it("语言偏好写入失败时保留本次切换并用左下角提示", async () => {
    const user = userEvent.setup();
    render(
      <ErrorToastProvider>
        <LanguageProvider><LanguageProbe /></LanguageProvider>
      </ErrorToastProvider>,
    );
    vi.spyOn(Storage.prototype, "setItem").mockImplementationOnce(() => {
      throw new Error("storage denied");
    });

    await user.click(screen.getByRole("button", { name: "简体中文" }));

    expect(screen.getByText("zh-CN:设置:主页路由")).toBeInTheDocument();
    expect(within(screen.getByTestId("error-toast-viewport")).getByRole("alert"))
      .toHaveTextContent("语言已在本次会话生效，但无法保存到下次启动");
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
