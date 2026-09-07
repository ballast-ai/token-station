import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { LanguageBoundary } from "./LanguageProvider";
import WebSearchSettings from "./WebSearchSettings";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const providers = [{ name: "search-provider", provider: "openai-compatible", base_url: "https://example.com/v1", models: ["any-search-model"], has_auth: true }];

describe("Web Search settings", () => {
  beforeEach(() => { vi.clearAllMocks(); localStorage.setItem("token-station-language", "en"); });
  it("loads the independent target and saves clearing it without changing the chat route", async () => {
    vi.mocked(invoke).mockImplementation(async (command) => command === "get_web_search_target" ? { upstream: "search-provider", model: "any-search-model" } : { settings: {} });
    const onSaved = vi.fn();
    const user = userEvent.setup();
    render(<LanguageBoundary><WebSearchSettings providers={providers} serveRunning onSaved={onSaved} /></LanguageBoundary>);
    await waitFor(() => expect(screen.getByRole("combobox", { name: "Search provider" })).not.toBeDisabled());
    expect(screen.getByRole("combobox", { name: "Search model" })).toHaveTextContent("any-search-model");
    await user.click(screen.getByRole("combobox", { name: "Search provider" }));
    await user.click(screen.getByRole("option", { name: "Use the current route's native search" }));
    await user.click(screen.getByRole("button", { name: "Save search settings" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("set_web_search_target", { upstream: "", model: "" }));
    expect(onSaved).toHaveBeenCalledOnce();
  });
  it("keeps saving disabled when settings cannot be read", async () => {
    vi.mocked(invoke).mockRejectedValue(new Error("unavailable"));
    render(<LanguageBoundary><WebSearchSettings providers={providers} serveRunning={false} onSaved={vi.fn()} /></LanguageBoundary>);
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("get_web_search_target"));
    expect(screen.getByRole("button", { name: "Save search settings" })).toBeDisabled();
  });
});
