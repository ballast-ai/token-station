import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  addProvider,
  applyHomeRouteToAllAgents,
  applyAgentPlan,
  applySnapshotRestore,
  checkUpgrade,
  discoverProviderModels,
  deleteProfile,
  editProvider,
  getPlugins,
  getRecentReceipts,
  getRuntimeState,
  getRouterTable,
  getState,
  getStats,
  listAgentRegistry,
  listAgentSnapshots,
  listenServeState,
  planAgentConnection,
  planAgentDisconnect,
  planSnapshotRestore,
  mountAgentProfile,
  previewProviderRemoval,
  removeProvider,
  restoreProvider,
  saveHomeRouteAsProfile,
  saveConfig,
  saveAgentRoutes,
  scanAgents,
  serveStart,
  serveStop,
  setAdminEndpoint,
  setSettings,
  setAgentRouteMode,
  setAgentTier,
  setTier,
  testProvider,
  updateProviderModels,
} from "./api";
import type { ServeView } from "./api";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(),
}));

const invokeMock = vi.mocked(invoke);
const listenMock = vi.mocked(listen);
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

function serveFixture(overrides: Partial<ServeView> = {}): ServeView {
  return {
    phase: "stopped", app_runtime: "stopped", listener_reachable: false,
    agent_connected: false, running_revision: null, instance_id: null,
    listen: "127.0.0.1:9999", virtual_key: null, error: null, ...overrides,
  };
}

beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockResolvedValue(undefined);
  listenMock.mockReset();
  listenMock.mockResolvedValue(vi.fn());
});

describe("structured Agent IPC", () => {
  it("lists Registry metadata without triggering discovery arguments", async () => {
    await listAgentRegistry();
    expect(invokeMock).toHaveBeenCalledWith("list_agent_registry");
  });

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

  it("binds a plan request to the version shown to the user when available", async () => {
    await planAgentConnection("claude-code", "/opt/claude", { expectedVersion: "2.1.210" });

    expect(invokeMock).toHaveBeenCalledWith("plan_agent_connection", {
      agentId: "claude-code",
      installationPath: "/opt/claude",
      expectedVersion: "2.1.210",
    });
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
  it("subscribes to the stable proxy lifecycle event", async () => {
    const handler = vi.fn();
    await listenServeState(handler);
    expect(listenMock).toHaveBeenCalledWith("serve-state-changed", expect.any(Function));
  });

  it.each([
    ["get state", () => getState(), "get_state", undefined],
    ["get runtime facts", () => getRuntimeState(), "get_runtime_state", undefined],
    ["add provider", () => addProvider("p", "https://p/v1", ["m"], "k"), "add_provider", { name: "p", baseUrl: "https://p/v1", models: ["m"], apiKey: "k" }],
    ["edit provider", () => editProvider("p", "https://p/v1", null), "edit_provider", { name: "p", baseUrl: "https://p/v1", apiKey: null }],
    ["preview provider removal", () => previewProviderRemoval("p"), "preview_provider_removal", { name: "p" }],
    ["remove provider", () => removeProvider("p"), "remove_provider", { name: "p" }],
    ["restore provider", () => restoreProvider("p"), "restore_provider", { name: "p" }],
    ["discover models", () => discoverProviderModels("p", "https://p/v1", null), "discover_provider_models", { name: "p", baseUrl: "https://p/v1", apiKey: null }],
    ["test provider", () => testProvider("p"), "test_provider", { name: "p" }],
    ["update models", () => updateProviderModels("p", ["a", "b"]), "update_provider_models", { name: "p", models: ["a", "b"] }],
    ["set tier", () => setTier("high", "p", "m"), "set_tier", { slot: "high", upstream: "p", model: "m" }],
    ["set Agent route mode", () => setAgentRouteMode("codex", "custom"), "set_agent_route_mode", { agentId: "codex", mode: "custom" }],
    ["set Agent tier", () => setAgentTier("codex", "high", "p", "m"), "set_agent_tier", { agentId: "codex", slot: "high", upstream: "p", model: "m" }],
    ["save route profile", () => saveHomeRouteAsProfile("daily"), "save_home_route_as_profile", { name: "daily" }],
    ["mount route profile", () => mountAgentProfile("codex", "daily"), "mount_agent_profile", { agentId: "codex", profile: "daily" }],
    ["delete route profile", () => deleteProfile("daily"), "delete_profile", { name: "daily" }],
    ["save", () => saveConfig(), "save_config", undefined],
    ["save Agent routes", () => saveAgentRoutes(), "save_agent_routes", undefined],
    ["apply home to all Agents", () => applyHomeRouteToAllAgents(), "apply_home_route_to_all_agents", undefined],
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
        : url.includes("receipts")
          ? [{ request_id: "receipt-1", attempt_records: [], conversion_reports: [] }]
        : url.includes("router-table")
          ? { rules: [], hint_routes: [], bands: [], pools: [] }
          : { dir: "/plugins", agent: "a", dialects: [], listing: "" };
      return new Response(JSON.stringify(body), { status: 200 });
    });
    setAdminEndpoint(serveFixture({ phase: "running", app_runtime: "running", listener_reachable: true, virtual_key: "virtual" }));

    await getStats("24h", "model");
    const receipts = await getRecentReceipts(5);
    await getRouterTable();
    await getPlugins();

    expect(receipts[0]?.request_id).toBe("receipt-1");
    expect(fetchMock).toHaveBeenCalledTimes(4);
    expect(fetchMock.mock.calls[0]?.[0]).toBe("http://127.0.0.1:9999/admin/stats?since=24h&by=model");
    expect(fetchMock.mock.calls[0]?.[1]).toEqual({ headers: { authorization: "Bearer virtual" } });
    expect(fetchMock.mock.calls[1]?.[0]).toBe("http://127.0.0.1:9999/admin/receipts");
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it("fails clearly in browser mode when HTTP is unavailable or stopped", async () => {
    vi.spyOn(globalThis, "fetch").mockRejectedValue(new Error("offline"));
    setAdminEndpoint(serveFixture({ phase: "running", app_runtime: "running", listener_reachable: true }));
    await expect(getStats("all", null)).rejects.toThrow("无法连接本地代理");

    setAdminEndpoint(serveFixture());
    await expect(getRouterTable()).rejects.toThrow("无法连接本地代理");

    setAdminEndpoint(serveFixture({ phase: "starting", app_runtime: "running", virtual_key: "stale" }));
    await expect(getPlugins()).rejects.toThrow("无法连接本地代理");
  });
});
