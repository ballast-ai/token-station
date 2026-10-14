import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import LocalCodexPanel from "./LocalCodexPanel";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("./LanguageProvider", () => ({ useLocalizedCopy: () => ({ copy: (en: string) => en }) }));
const account = { id: "local-1", label: "Local account", email: "person@example.test", state: "ready", account_id: "account-1", client_version: "1.0.0" };
function commands(overrides: Record<string, unknown> = {}) {
  const values: Record<string, unknown> = {
    codex_accounts_list: [], codex_account_discover: account, codex_connect_provider: "codex_local-1", codex_account_disconnect: null,
    codex_account_models: [{ id: "live-model", name: "Live model", context_window: null, supports_images: null }], ...overrides,
  };
  vi.mocked(invoke).mockImplementation(async command => values[command] as never);
}
describe("Local Codex connection", () => {
  beforeEach(() => { vi.mocked(invoke).mockReset(); commands(); });
  it("loads references without login or inference", async () => {
    render(<LocalCodexPanel />);
    await waitFor(() => expect(screen.getByRole("button", { name: "Use local Codex login" })).toBeEnabled());
    expect(invoke).toHaveBeenCalledTimes(1); expect(invoke).toHaveBeenCalledWith("codex_accounts_list");
    expect(screen.getByText(/Disconnecting here does not sign Codex out/)).toBeInTheDocument();
  });
  it("connects the local login in one action", async () => {
    const changed = vi.fn(); render(<LocalCodexPanel onChanged={changed} />);
    const button = screen.getByRole("button", { name: "Use local Codex login" });
    await waitFor(() => expect(button).toBeEnabled()); fireEvent.click(button);
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("codex_connect_provider", { accountId: null }));
    await waitFor(() => expect(changed).toHaveBeenCalledOnce());
    expect(await screen.findByText(/Select the provider for your Agents/)).toBeInTheDocument();
  });
  it.each([
    ["missing", "Sign in to Codex on this computer"], ["api_key_only", "uses an API key"], ["expired", "Renew the login in Codex"],
    ["unsupported_storage", "credential storage is not supported"], ["account_changed", "account has changed"], ["invalid_source", "login data could not be read"],
  ])("explains %s", async (state, text) => {
    commands({ codex_account_discover: { ...account, state } }); render(<LocalCodexPanel />);
    const check = screen.getByRole("button", { name: "Check local login" });
    await waitFor(() => expect(check).toBeEnabled()); fireEvent.click(check);
    expect(await screen.findByText(new RegExp(text))).toBeInTheDocument();
    expect(invoke).not.toHaveBeenCalledWith("codex_connect_provider", expect.anything());
  });
  it("refreshes live models and disconnects the reference", async () => {
    commands({ codex_accounts_list: [account] }); render(<LocalCodexPanel provider={{ name: "codex_local-1", provider: "codex-plan", credential_reference: account.id, base_url: "https://chatgpt.com/backend-api/codex", has_auth: true, models: ["managed-model"] }} />);
    fireEvent.click(await screen.findByRole("button", { name: "Refresh models" }));
    expect(await screen.findByText("Live model")).toBeInTheDocument();
    expect(invoke).toHaveBeenCalledWith("codex_account_models", { accountId: "local-1" });
    fireEvent.click(screen.getByRole("button", { name: "Disconnect reference" }));
    expect(invoke).not.toHaveBeenCalledWith("codex_account_disconnect", expect.anything());
    fireEvent.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Disconnect reference" }));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("codex_account_disconnect", { accountId: "local-1" }));
    expect(await screen.findByText(/Codex remains signed in/)).toBeInTheDocument();
  });
  it("shows a recovery state when one-click connection fails", async () => {
    commands({ codex_account_discover: { ...account, state: "missing", label: "Codex" } });
    const fallback = vi.mocked(invoke).getMockImplementation()!;
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "codex_connect_provider") throw new Error("refresh_token=SECRET_FIXTURE");
      return fallback(command, args);
    });
    render(<LocalCodexPanel />);
    const button = screen.getByRole("button", { name: "Use local Codex login" });
    await waitFor(() => expect(button).toBeEnabled()); fireEvent.click(button);
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not connect the local Codex account");
    expect(screen.getByText(/Sign in to Codex on this computer/)).toBeInTheDocument();
    expect(document.body.textContent).not.toContain("SECRET_FIXTURE");
  });
  it("disables changed accounts and redacts backend errors", async () => {
    commands({ codex_accounts_list: [{ ...account, state: "account_changed" }] }); render(<LocalCodexPanel provider={{ name: "codex_local-1", provider: "codex-plan", credential_reference: account.id, base_url: "https://chatgpt.com/backend-api/codex", has_auth: true, models: ["managed-model"] }} />);
    expect(await screen.findByRole("button", { name: "Refresh models" })).toBeDisabled();
    vi.mocked(invoke).mockRejectedValue("access_token=SECRET_FIXTURE");
    fireEvent.click(screen.getByRole("button", { name: "Check local login" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not check the local Codex login");
    expect(document.body.textContent).not.toContain("SECRET_FIXTURE");
  });
});


it("shows only the local reference bound to the managed provider", async () => {
  commands({ codex_accounts_list: [account, { ...account, id: "local-2", label: "Second local reference" }] });
  render(<LocalCodexPanel provider={{ name: "custom-local", provider: "codex-plan", credential_reference: "local-2", base_url: "https://chatgpt.com/backend-api/codex", has_auth: true, models: ["bound-model"] }} />);
  expect(await screen.findByText("Second local reference")).toBeInTheDocument();
  expect(screen.queryByText("Local account")).toBeNull();
  expect(screen.getByText("bound-model")).toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "Refresh models" }));
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("codex_account_models", { accountId: "local-2" }));
  expect(invoke).not.toHaveBeenCalledWith("codex_connect_provider", expect.anything());
});


it("keeps local setup focused on connecting the intended account", async () => {
  commands({ codex_accounts_list: [account] });
  const connected = vi.fn();
  render(<LocalCodexPanel onConnected={connected} />);
  expect(await screen.findByText("Local account")).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Refresh models" })).toBeNull();
  expect(screen.queryByRole("button", { name: "Disconnect reference" })).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "Use this account" }));
  await waitFor(() => expect(connected).toHaveBeenCalledOnce());
  expect(invoke).toHaveBeenCalledWith("codex_connect_provider", { accountId: "local-1" });
});
