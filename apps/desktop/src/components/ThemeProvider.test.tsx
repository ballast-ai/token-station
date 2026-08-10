import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  THEME_STORAGE_KEY,
  ThemeProvider,
  useTheme,
  type ResolvedTheme,
} from "./ThemeProvider";

const { invokeMock } = vi.hoisted(() => ({ invokeMock: vi.fn() }));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: invokeMock,
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
  invokeMock.mockReset();
  Reflect.deleteProperty(window, "__TAURI_INTERNALS__");
  window.localStorage.clear();
  document.documentElement.classList.remove("light", "dark");
  systemTheme = "light";
  installMatchMedia();
});

describe("ThemeProvider", () => {
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
      expect(invokeMock).toHaveBeenCalledWith("set_dock_theme_icon", { theme: "dark" }),
    );

    await user.click(screen.getByRole("button", { name: "Light" }));
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("set_dock_theme_icon", { theme: "light" }),
    );
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
