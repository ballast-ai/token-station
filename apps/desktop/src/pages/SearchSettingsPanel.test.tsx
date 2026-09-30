import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import SearchSettingsPanel from "./SearchSettingsPanel";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));
vi.mock("../components/LanguageProvider", () => ({ useLanguage: () => ({ copy: (en: string) => en }) }));
const status = { settings: { enabled: false, mode: "auto", engine: "bing" }, chrome_available: true, busy: false };

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
  it("requires a gateway check before claiming search is ready", async () => {
    let finish!: (value: unknown) => void;
    vi.mocked(invoke).mockResolvedValueOnce(status).mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
    render(<SearchSettingsPanel />);
    await waitFor(() => expect(screen.getByRole("switch")).toBeEnabled());
    fireEvent.click(screen.getByRole("switch"));
    expect(invoke).toHaveBeenCalledWith("save_search_settings", { settings: { enabled: true, mode: "auto", engine: "bing" } });
    expect(screen.getByRole("switch")).not.toBeChecked();
    expect(screen.queryByText(/Gateway search verified/)).not.toBeInTheDocument();
    finish({ status: { ...status, settings: { ...status.settings, enabled: true } }, verified: true, managed_codex_updated: 0 });
    expect(await screen.findByText(/Gateway search verified/)).toBeInTheDocument();
    expect(screen.getByRole("switch")).toBeChecked();
    expect(screen.getByText(/Connect Codex on the Agents page/)).toBeInTheDocument();
  });
  it("keeps search off when activation fails", async () => {
    vi.mocked(invoke).mockResolvedValueOnce(status).mockRejectedValueOnce("The model did not complete a search.");
    render(<SearchSettingsPanel />);
    await waitFor(() => expect(screen.getByRole("switch")).toBeEnabled());
    fireEvent.click(screen.getByRole("switch"));
    expect(await screen.findByRole("alert")).toHaveTextContent("did not complete");
    expect(screen.getByRole("switch")).not.toBeChecked();
  });
  it("can disable interception even after Chrome was removed", async () => {
    vi.mocked(invoke).mockResolvedValueOnce({ ...status, chrome_available: false, settings: { ...status.settings, enabled: true } }).mockResolvedValueOnce({ status, verified: false, managed_codex_updated: 0 });
    render(<SearchSettingsPanel />);
    await waitFor(() => expect(screen.getByRole("switch")).toBeChecked());
    fireEvent.click(screen.getByRole("switch"));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("save_search_settings", { settings: { enabled: false, mode: "auto", engine: "bing" } }));
  });
  it("permits native search without Chrome", async () => {
    vi.mocked(invoke).mockResolvedValueOnce({ ...status, chrome_available: false, settings: { ...status.settings, mode: "native" } });
    render(<SearchSettingsPanel />);
    await waitFor(() => expect(screen.getByRole("switch")).toBeEnabled());
    expect(screen.getByRole("combobox", { name: "Search mode" })).toBeInTheDocument();
  });
  it("saves the selected mode before activation", async () => {
    vi.mocked(invoke).mockResolvedValueOnce(status).mockResolvedValueOnce({ status: { ...status, settings: { ...status.settings, mode: "native" } }, verified: false, managed_codex_updated: 0 });
    render(<SearchSettingsPanel />);
    const selector = await screen.findByRole("combobox", { name: "Search mode" });
    fireEvent.change(selector, { target: { value: "native" } });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("save_search_settings", { settings: { enabled: false, mode: "native", engine: "bing" } }));
  });

});
