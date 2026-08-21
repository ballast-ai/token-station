import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  LANGUAGE_STORAGE_KEY,
  LANGUAGE_PREFERENCE_VERSION_KEY,
  LanguageProvider,
  useLanguage,
} from "./LanguageProvider";
import { ErrorToastProvider } from "./ErrorToast";

function LanguageProbe() {
  const { language, setLanguage, t, copy } = useLanguage();
  return (
    <div>
      <output>{`${language}:${t("settings.title")}:${copy("Home routing", "主页路由", "首頁路由", "ホームルーティング")}`}</output>
      <button type="button" onClick={() => setLanguage("en")}>English</button>
      <button type="button" onClick={() => setLanguage("zh-CN")}>简体中文</button>
      <button type="button" onClick={() => setLanguage("zh-TW")}>繁體中文</button>
      <button type="button" onClick={() => setLanguage("ja")}>日本語</button>
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
    setBrowserLanguages(["fr-FR", "zh-Hans-CN", "en-US"]);

    render(
      <LanguageProvider>
        <LanguageProbe />
      </LanguageProvider>,
    );

    expect(screen.getByText("zh-CN:设置:主页路由")).toBeInTheDocument();
    expect(document.documentElement).toHaveAttribute("lang", "zh-CN");
    expect(window.localStorage.getItem(LANGUAGE_STORAGE_KEY)).toBe("zh-CN");
  });

  it.each([
    ["zh-TW", "zh-TW:設定:首頁路由"],
    ["zh-HK", "zh-TW:設定:首頁路由"],
    ["zh-Hant", "zh-TW:設定:首頁路由"],
    ["ja-JP", "ja:設定:ホームルーティング"],
  ])("detects %s on first launch", (browserLanguage, expected) => {
    setBrowserLanguages([browserLanguage, "en-US"]);

    render(
      <LanguageProvider>
        <LanguageProbe />
      </LanguageProvider>,
    );

    expect(screen.getByText(expected)).toBeInTheDocument();
  });

  it.each([
    ["zh-Hans-HK", "zh-CN:设置:主页路由"],
    ["zh-Hant-CN", "zh-TW:設定:首頁路由"],
  ])("gives an explicit script priority over the region in %s", (browserLanguage, expected) => {
    setBrowserLanguages([browserLanguage]);
    render(<LanguageProvider><LanguageProbe /></LanguageProvider>);
    expect(screen.getByText(expected)).toBeInTheDocument();
  });

  it.each(["ja-JP", "zh-HK"])(
    "preserves an unversioned legacy English preference on %s",
    (browserLanguage) => {
    setBrowserLanguages([browserLanguage]);
    window.localStorage.setItem(LANGUAGE_STORAGE_KEY, "en");
    render(<LanguageProvider><LanguageProbe /></LanguageProvider>);
    expect(screen.getByText("en:Settings:Home routing")).toBeInTheDocument();
    expect(window.localStorage.getItem(LANGUAGE_PREFERENCE_VERSION_KEY)).toBe("2");
    },
  );

  it("preserves a versioned explicit English selection on a Japanese system", () => {
    setBrowserLanguages(["ja-JP"]);
    window.localStorage.setItem(LANGUAGE_STORAGE_KEY, "en");
    window.localStorage.setItem(LANGUAGE_PREFERENCE_VERSION_KEY, "2");
    render(<LanguageProvider><LanguageProbe /></LanguageProvider>);
    expect(screen.getByText("en:Settings:Home routing")).toBeInTheDocument();
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
    setBrowserLanguages(["fr-FR", "de-DE", "en-GB"]);
    const { unmount } = render(
      <LanguageProvider>
        <LanguageProbe />
      </LanguageProvider>,
    );
    expect(screen.getByText("en:Settings:Home routing")).toBeInTheDocument();
    unmount();

    window.localStorage.clear();
    setBrowserLanguages(["fr-FR", "de-DE"]);
    render(
      <LanguageProvider>
        <LanguageProbe />
      </LanguageProvider>,
    );
    expect(screen.getByText("en:Settings:Home routing")).toBeInTheDocument();
  });

  it("restores and persists Traditional Chinese and Japanese selections", async () => {
    const user = userEvent.setup();
    render(
      <LanguageProvider>
        <LanguageProbe />
      </LanguageProvider>,
    );

    await user.click(screen.getByRole("button", { name: "繁體中文" }));
    expect(screen.getByText("zh-TW:設定:首頁路由")).toBeInTheDocument();
    expect(document.documentElement).toHaveAttribute("lang", "zh-TW");
    expect(window.localStorage.getItem(LANGUAGE_STORAGE_KEY)).toBe("zh-TW");

    await user.click(screen.getByRole("button", { name: "日本語" }));
    expect(screen.getByText("ja:設定:ホームルーティング")).toBeInTheDocument();
    expect(document.documentElement).toHaveAttribute("lang", "ja");
    expect(window.localStorage.getItem(LANGUAGE_STORAGE_KEY)).toBe("ja");
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
