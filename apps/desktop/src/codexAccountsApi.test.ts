import { beforeEach, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { autoConnectCodexProvider, connectCodexProvider, discoverCodexAccount, disconnectCodexAccount, getCodexModels, listCodexAccounts } from "./codexAccountsApi";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
beforeEach(() => vi.mocked(invoke).mockReset());
it("passes only opaque reference IDs across the IPC boundary", async () => {
  await listCodexAccounts(); await discoverCodexAccount(); await connectCodexProvider();
  await connectCodexProvider("local-1"); await getCodexModels("local-1"); await disconnectCodexAccount("local-1");
  expect(vi.mocked(invoke).mock.calls).toEqual([
    ["codex_accounts_list"], ["codex_account_discover"], ["codex_connect_provider", { accountId: null }],
    ["codex_connect_provider", { accountId: "local-1" }], ["codex_account_models", { accountId: "local-1" }],
    ["codex_account_disconnect", { accountId: "local-1" }],
  ]);
});

it("requests automatic discovery without transmitting credentials or source paths", async () => {
  vi.mocked(invoke).mockResolvedValueOnce(true);
  expect(await autoConnectCodexProvider()).toBe(true);
  expect(invoke).toHaveBeenCalledExactlyOnceWith("codex_auto_connect_provider");
});
