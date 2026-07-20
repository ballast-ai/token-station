import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  addProvider,
  applyAgentPlan,
  applySnapshotRestore,
  checkUpgrade,
  discoverProviderModels,
  getPlugins,
  getRouterTable,
  getState,
  getStats,
  listAgentSnapshots,
  planAgentConnection,
  planAgentDisconnect,
  planSnapshotRestore,
  removeProvider,
  saveConfig,
  scanAgents,
  serveStart,
  serveStop,
  setAdminEndpoint,
  setSettings,
  setTier,
  updateProviderModels,
} from "./api";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

const invokeMock = vi.mocked(invoke);
const forbiddenKeys = new Set([
  "patch",
  "patches",
  "targetPath",
  "target_config_path",
  "config",
  "content",
  "command",
  "argv",
  "executablePath",
]);

beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockResolvedValue(undefined);
});

describe("structured Agent IPC", () => {
  it("scans without renderer-controlled arguments", async () => {
    await scanAgents();
    expect(invokeMock).toHaveBeenCalledWith("scan_agents");
  });

  it.each([
    [
      "plan connection",
      () => planAgentConnection("claude-code", "/opt/claude"),
      "plan_agent_connection",
      { agentId: "claude-code", installationPath: "/opt/claude" },
    ],
    [
      "apply connection or disconnect",
      () => applyAgentPlan("operation", "confirmation"),
      "apply_agent_plan",
      { operationId: "operation", confirmationToken: "confirmation" },
    ],
    [
      "plan disconnect",
      () => planAgentDisconnect("codex", "/opt/codex"),
      "plan_agent_disconnect",
      { agentId: "codex", installationPath: "/opt/codex" },
    ],
    [
      "list metadata-only snapshots",
      () => listAgentSnapshots("opencode"),
      "list_agent_snapshots",
      { agentId: "opencode" },
    ],
    [
      "plan snapshot restore",
      () => planSnapshotRestore("snapshot"),
      "plan_snapshot_restore",
      { snapshotId: "snapshot" },
    ],
    [
      "apply snapshot restore through its dedicated endpoint",
      () => applySnapshotRestore("operation", "confirmation"),
      "apply_snapshot_restore",
      { operationId: "operation", confirmationToken: "confirmation" },
    ],
  ])("maps %s to an exact command payload", async (_label, call, command, payload) => {
    await call();
    expect(invokeMock).toHaveBeenCalledWith(command, payload);
    const sent = invokeMock.mock.calls[0]?.[1] as Record<string, unknown>;
    expect(Object.keys(sent).sort()).toEqual(Object.keys(payload).sort());
    expect(Object.keys(sent).some((key) => forbiddenKeys.has(key))).toBe(false);
  });

  it("does not expose arbitrary patch, target, config, or command parameters", () => {
    if (false) {
      // @ts-expect-error apply accepts no renderer-provided patch
      applyAgentPlan("operation", "confirmation", { patch: [] });
      // @ts-expect-error plan accepts no renderer-provided target path
      planAgentConnection("claude-code", "/opt/claude", "/tmp/settings.json");
      // @ts-expect-error installation selection is required
      planAgentConnection("claude-code");
    }
    expect(true).toBe(true);
  });
});

describe("desktop API mapping and read-only HTTP data plane", () => {
  it.each([
    ["get state", () => getState(), "get_state", undefined],
    ["add provider", () => addProvider("p", "https://p/v1", ["m"], "k"), "add_provider", { name: "p", baseUrl: "https://p/v1", models: ["m"], apiKey: "k" }],
    ["remove provider", () => removeProvider("p"), "remove_provider", { name: "p" }],
    ["discover models", () => discoverProviderModels("p", "https://p/v1", null), "discover_provider_models", { name: "p", baseUrl: "https://p/v1", apiKey: null }],
    ["update models", () => updateProviderModels("p", ["a", "b"]), "update_provider_models", { name: "p", models: ["a", "b"] }],
    ["set tier", () => setTier("high", "p", "m"), "set_tier", { slot: "high", upstream: "p", model: "m" }],
    ["save", () => saveConfig(), "save_config", undefined],
    ["start", () => serveStart(), "serve_start", undefined],
    ["stop", () => serveStop(), "serve_stop", undefined],
    ["settings", () => setSettings(false, true), "set_settings", { auth: false, metrics: true }],
    ["upgrade", () => checkUpgrade(), "check_upgrade", undefined],
  ])("maps %s to the stable IPC contract", async (_label, call, command, payload) => {
    await call();
    if (payload === undefined) expect(invokeMock).toHaveBeenCalledWith(command);
    else expect(invokeMock).toHaveBeenCalledWith(command, payload);
  });

  it("reads every public data view over authenticated loopback HTTP", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockImplementation(async (input) => {
      const url = String(input);
      const body = url.includes("stats")
        ? { total: {}, groups: [], by: null, empty: true }
        : url.includes("router-table")
          ? { rules: [], hint_routes: [], bands: [], pools: [] }
          : { dir: "/plugins", agent: "a", dialects: [], listing: "" };
      return new Response(JSON.stringify(body), { status: 200 });
    });
    setAdminEndpoint({ running: true, listen: "127.0.0.1:9999", virtual_key: "virtual" });

    await getStats("24h", "model");
    await getRouterTable();
    await getPlugins();

    expect(fetchMock).toHaveBeenCalledTimes(3);
    expect(fetchMock.mock.calls[0]?.[0]).toBe("http://127.0.0.1:9999/admin/stats?since=24h&by=model");
    expect(fetchMock.mock.calls[0]?.[1]).toEqual({ headers: { authorization: "Bearer virtual" } });
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it("fails clearly in browser mode when HTTP is unavailable or stopped", async () => {
    vi.spyOn(globalThis, "fetch").mockRejectedValue(new Error("offline"));
    setAdminEndpoint({ running: true, listen: "127.0.0.1:9999", virtual_key: null });
    await expect(getStats("all", null)).rejects.toThrow("无法连接本地代理");

    setAdminEndpoint({ running: false, listen: "127.0.0.1:9999", virtual_key: null });
    await expect(getRouterTable()).rejects.toThrow("无法连接本地代理");
  });
});
