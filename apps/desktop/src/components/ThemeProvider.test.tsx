import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { setDockThemeIcon } from "../api";
import {
  THEME_STORAGE_KEY,
  ThemeProvider,
  useTheme,
  type ResolvedTheme,
} from "./ThemeProvider";
import { ErrorToastProvider } from "./ErrorToast";

const { setDockThemeIconMock, setWindowThemeMock } = vi.hoisted(() => ({
  setDockThemeIconMock: vi.fn(),
  setWindowThemeMock: vi.fn(),
}));

vi.mock("../api", () => ({
  setDockThemeIcon: setDockThemeIconMock,
}));

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ setTheme: setWindowThemeMock }),
}));

type ChangeListener = (event: MediaQueryListEvent) => void;

let systemTheme: ResolvedTheme;
let changeListeners: Set<ChangeListener>;

function installMatchMedia() {
  changeListeners = new Set();
  vi.stubGlobal(
    "matchMedia",
    vi.fn().mockImplementation((query: string) => ({
      matches: systemTheme === "dark",
      media: query,
      onchange: null,
      addEventListener: (_type: string, listener: ChangeListener) => changeListeners.add(listener),
      removeEventListener: (_type: string, listener: ChangeListener) => changeListeners.delete(listener),
      addListener: vi.fn(),
      removeListener: vi.fn(),
      dispatchEvent: vi.fn(),
    })),
  );
}

function setSystemTheme(theme: ResolvedTheme) {
  systemTheme = theme;
  const event = { matches: theme === "dark" } as MediaQueryListEvent;
  for (const listener of changeListeners) listener(event);
}

function ThemeProbe() {
  const { theme, resolvedTheme, setTheme } = useTheme();
  return (
    <div>
      <output>{`${theme}:${resolvedTheme}`}</output>
      <button onClick={() => setTheme("light")}>Light</button>
      <button onClick={() => setTheme("dark")}>Dark</button>
      <button onClick={() => setTheme("system")}>System</button>
    </div>
  );
}

beforeEach(() => {
  setDockThemeIconMock.mockReset().mockResolvedValue(undefined);
  setWindowThemeMock.mockReset().mockResolvedValue(undefined);
  Reflect.deleteProperty(window, "__TAURI_INTERNALS__");
  window.localStorage.clear();
  document.documentElement.classList.remove("light", "dark");
  systemTheme = "light";
  installMatchMedia();
});

describe("ThemeProvider", () => {
  it("grants the main window permission to synchronize its native theme", () => {
    const capability = JSON.parse(
      readFileSync(
        resolve(process.cwd(), "src-tauri/capabilities/default.json"),
        "utf8",
      ),
    ) as { permissions?: string[] };

    expect(capability.permissions).toContain("core:window:allow-set-theme");
  });

  it("does not require AppKit to return the same NSImage allocation", () => {
    const backendSource = readFileSync(
      resolve(process.cwd(), "src-tauri/src/lib.rs"),
      "utf8",
    );

    expect(backendSource).toContain("applicationIconImage()");
    expect(backendSource).not.toContain("std::ptr::eq(&*image, &*applied_image)");
  });

  it("defaults to the system preference and keeps the root class in sync", () => {
    systemTheme = "dark";
    installMatchMedia();

    render(
      <ThemeProvider>
        <ThemeProbe />
      </ThemeProvider>,
    );

    expect(screen.getByText("system:dark")).toBeInTheDocument();
    expect(document.documentElement).toHaveClass("dark");
    expect(document.documentElement).not.toHaveClass("light");
    expect(window.localStorage.getItem(THEME_STORAGE_KEY)).toBe("system");
  });

  it("restores and persists an explicit theme selection", async () => {
    window.localStorage.setItem(THEME_STORAGE_KEY, "dark");
    const user = userEvent.setup();

    render(
      <ThemeProvider>
        <ThemeProbe />
      </ThemeProvider>,
    );

    expect(screen.getByText("dark:dark")).toBeInTheDocument();
    expect(document.documentElement).toHaveClass("dark");

    await user.click(screen.getByRole("button", { name: "Light" }));
    expect(screen.getByText("light:light")).toBeInTheDocument();
    expect(document.documentElement).toHaveClass("light");
    expect(document.documentElement).not.toHaveClass("dark");
    expect(window.localStorage.getItem(THEME_STORAGE_KEY)).toBe("light");
  });

  it("tracks matchMedia changes only while system mode is active", async () => {
    const user = userEvent.setup();
    render(
      <ThemeProvider>
        <ThemeProbe />
      </ThemeProvider>,
    );

    expect(changeListeners).toHaveLength(1);
    act(() => setSystemTheme("dark"));
    expect(screen.getByText("system:dark")).toBeInTheDocument();
    expect(document.documentElement).toHaveClass("dark");

    await user.click(screen.getByRole("button", { name: "Light" }));
    expect(changeListeners).toHaveLength(0);
    act(() => setSystemTheme("dark"));
    expect(screen.getByText("light:light")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "System" }));
    expect(changeListeners).toHaveLength(1);
    expect(screen.getByText("system:dark")).toBeInTheDocument();
  });

  it("keeps the macOS Dock icon in sync with the resolved theme", async () => {
    Object.defineProperty(window, "__TAURI_INTERNALS__", {
      configurable: true,
      value: {},
    });
    systemTheme = "dark";
    installMatchMedia();
    const user = userEvent.setup();

    render(
      <ThemeProvider>
        <ThemeProbe />
      </ThemeProvider>,
    );

    await waitFor(() =>
      expect(setDockThemeIcon).toHaveBeenCalledWith("dark"),
    );

    await user.click(screen.getByRole("button", { name: "Light" }));
    await waitFor(() =>
      expect(setDockThemeIcon).toHaveBeenCalledWith("light"),
    );
  });

  it("keeps the page theme active when the native Dock update fails", async () => {
    Object.defineProperty(window, "__TAURI_INTERNALS__", {
      configurable: true,
      value: {},
    });
    setDockThemeIconMock.mockRejectedValue(new Error("native Dock update failed"));
    const user = userEvent.setup();

    render(<ErrorToastProvider><ThemeProvider><ThemeProbe /></ThemeProvider></ErrorToastProvider>);

    await user.click(screen.getByRole("button", { name: "Dark" }));
    expect(screen.getByText("dark:dark")).toBeInTheDocument();
    expect(document.documentElement).toHaveClass("dark");
    expect(await within(screen.getByTestId("error-toast-viewport")).findByRole("alert"))
      .toHaveTextContent("macOS appearance did not fully synchronize");
  });

  it("主题偏好写入失败时保留本次切换并用左下角提示", async () => {
    const user = userEvent.setup();
    render(<ErrorToastProvider><ThemeProvider><ThemeProbe /></ThemeProvider></ErrorToastProvider>);
    vi.spyOn(Storage.prototype, "setItem").mockImplementationOnce(() => {
      throw new Error("storage denied");
    });

    await user.click(screen.getByRole("button", { name: "Dark" }));

    expect(screen.getByText("dark:dark")).toBeInTheDocument();
    expect(within(screen.getByTestId("error-toast-viewport")).getByRole("alert"))
      .toHaveTextContent("theme changed for this session");
  });

  it("ignores invalid persisted values", () => {
    window.localStorage.setItem(THEME_STORAGE_KEY, "sepia");
    render(
      <ThemeProvider>
        <ThemeProbe />
      </ThemeProvider>,
    );

    expect(screen.getByText("system:light")).toBeInTheDocument();
    expect(window.localStorage.getItem(THEME_STORAGE_KEY)).toBe("system");
  });

  it("rejects useTheme outside its provider", () => {
    expect(() => render(<ThemeProbe />)).toThrow("useTheme must be used within ThemeProvider");
  });
});
