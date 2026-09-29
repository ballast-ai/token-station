import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import SearchSettingsPanel from "./SearchSettingsPanel";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));
vi.mock("../components/LanguageProvider", () => ({ useLanguage: () => ({ copy: (en: string) => en }) }));
const status = { settings: { enabled: false, engine: "bing" }, chrome_available: true, busy: false };

describe("browser search preview", () => {
  beforeEach(() => vi.mocked(invoke).mockReset());
  it("tests a real query without enabling interception", async () => {
    vi.mocked(invoke).mockResolvedValueOnce(status).mockResolvedValueOnce({ results: [{ title: "Python docs", url: "https://docs.python.org/", snippet: "Official documentation" }], elapsed_ms: 1234 });
    render(<SearchSettingsPanel />);
    const button = await screen.findByRole("button", { name: "Test search" });
    await waitFor(() => expect(button).toBeEnabled());
    fireEvent.click(button);
    expect(await screen.findByText("Python docs")).toBeInTheDocument();
    expect(invoke).toHaveBeenCalledWith("test_browser_search", { query: "Python official documentation" });
    expect(invoke).not.toHaveBeenCalledWith("save_search_settings", expect.anything());
    expect(screen.getByRole("switch")).not.toBeChecked();
  });
  it("shows a backend error without invented results", async () => {
    vi.mocked(invoke).mockResolvedValueOnce(status).mockRejectedValueOnce("The search engine requires human verification.");
    render(<SearchSettingsPanel />);
    const button = await screen.findByRole("button", { name: "Test search" });
    await waitFor(() => expect(button).toBeEnabled());
    fireEvent.click(button);
    expect(await screen.findByRole("alert")).toHaveTextContent("human verification");
  });
  it("can disable interception even after Chrome was removed", async () => {
    vi.mocked(invoke).mockResolvedValueOnce({ ...status, chrome_available: false, settings: { ...status.settings, enabled: true } }).mockResolvedValueOnce(status);
    render(<SearchSettingsPanel />);
    await waitFor(() => expect(screen.getByRole("switch")).toBeChecked());
    fireEvent.click(screen.getByRole("switch"));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("save_search_settings", { settings: { enabled: false, engine: "bing" } }));
  });
});
