import {
  createContext,
  useContext,
  useEffect,
  useLayoutEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { setDockThemeIcon } from "../api";
import { useErrorToast } from "./ErrorToast";

export type Theme = "light" | "dark" | "system";
export type ResolvedTheme = Exclude<Theme, "system">;

export const THEME_STORAGE_KEY = "token-station-theme";

type ThemeContextValue = {
  theme: Theme;
  resolvedTheme: ResolvedTheme;
  setTheme: (theme: Theme) => void;
};

const ThemeContext = createContext<ThemeContextValue | null>(null);

function isTheme(value: string | null): value is Theme {
  return value === "light" || value === "dark" || value === "system";
}

function storedTheme(): Theme {
  if (typeof window === "undefined") return "system";
  try {
    const value = window.localStorage.getItem(THEME_STORAGE_KEY);
    return isTheme(value) ? value : "system";
  } catch {
    return "system";
  }
}

function systemTheme(): ResolvedTheme {
  if (typeof window === "undefined" || typeof window.matchMedia !== "function") return "light";
  return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

export function ThemeProvider({ children }: { children: ReactNode }) {
  const { showError } = useErrorToast();
  const [theme, setTheme] = useState<Theme>(storedTheme);
  const [preferredTheme, setPreferredTheme] = useState<ResolvedTheme>(systemTheme);

  useEffect(() => {
    try {
      window.localStorage.setItem(THEME_STORAGE_KEY, theme);
    } catch {
      showError(
        document.documentElement.lang === "zh-CN"
          ? "主题已在本次会话生效，但无法保存到下次启动。"
          : "The theme changed for this session, but it could not be saved for the next launch.",
        "theme-storage",
      );
    }
  }, [showError, theme]);

  useEffect(() => {
    if (theme !== "system" || typeof window.matchMedia !== "function") return;

    const media = window.matchMedia("(prefers-color-scheme: dark)");
    const syncPreference = (matches: boolean) => setPreferredTheme(matches ? "dark" : "light");
    const handleChange = (event: MediaQueryListEvent) => syncPreference(event.matches);

    syncPreference(media.matches);
    media.addEventListener("change", handleChange);
    return () => media.removeEventListener("change", handleChange);
  }, [theme]);

  const resolvedTheme = theme === "system" ? preferredTheme : theme;

  useLayoutEffect(() => {
    document.documentElement.classList.remove("light", "dark");
    document.documentElement.classList.add(resolvedTheme);
    if ("__TAURI_INTERNALS__" in window) {
      const reportSyncFailure = () => showError(
        document.documentElement.lang === "zh-CN"
          ? "界面主题已切换，但 macOS 外观没有完全同步。"
          : "The interface theme changed, but the macOS appearance did not fully synchronize.",
        "theme-native-sync",
      );
      void getCurrentWindow().setTheme(resolvedTheme).catch(reportSyncFailure);
      void setDockThemeIcon(resolvedTheme).catch(reportSyncFailure);
    }
  }, [resolvedTheme, showError]);

  const value = useMemo(
    () => ({ theme, resolvedTheme, setTheme }),
    [theme, resolvedTheme],
  );

  return <ThemeContext.Provider value={value}>{children}</ThemeContext.Provider>;
}

export function ThemeBoundary({ children }: { children: ReactNode }) {
  const value = useContext(ThemeContext);
  if (value) return children;
  return <ThemeProvider>{children}</ThemeProvider>;
}

export function useTheme(): ThemeContextValue {
  const value = useContext(ThemeContext);
  if (!value) throw new Error("useTheme must be used within ThemeProvider");
  return value;
}
