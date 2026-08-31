import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  addProvider,
  addManagedEnterpriseRoute,
  addFreeProvider,
  setLocalRouting,
  applyHomeRouteToAllAgents,
  applyAgentPlan,
  applySnapshotRestore,
  checkDesktopUpdate,
  discoverProviderModels,
  deleteProfile,
  discardAgentPlan,
  editProvider,
  getPlugins,
  getEgress,
  getAgentDrift,
  getAgentBackupDirectory,
  getAgentBudgets,
  getPriceTable,
  getRecentReceipts,
  getRuntimeState,
  getCachedAgentViews,
  getRouterTable,
  getState,
  getStats,
  listAgentRegistry,
  listFreeProviderPresets,
  listPublicProviderModels,
  listAgentSnapshots,
  listenServeState,
  listenDesktopUpdateProgress,
  planAgentConnection,
  planAgentDisconnect,
  planSnapshotRestore,
  mountAgentProfile,
  previewProviderRemoval,
  purgeDeletedProviders,
  removeProvider,
  restoreProvider,
  saveHomeRouteAsProfile,
  saveConfig,
  saveAgentRoutes,
  scanAgents,
  ensureServeRunning,
  serveStart,
  serveStop,
  verifyEnterpriseRoute,
  setAdminEndpoint,
  setSettings,
  setAgentRouteMode,
  setAgentBudget,
  setModelPrice,
  suggestModelPrice,
  setAgentTier,
  setDirectRoute,
  setRoutingMode,
  setTier,
  testProvider,
  cancelModelTestChat,
  testModelChatStream,
  setProviderModelVision,
  setProviderModelLimits,
  updateProviderModels,
  removeAgentBudget,
  removeModelPrice,
  installDesktopUpdateAndRestart,
  exportRecoveryBundle,
  getRecoveryDiagnostics,
  getRecoveryState,
  openRecoveryFolder,
  openAgentBackupDirectory,
  recordFrontendDiagnostic,
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
    listen: "127.0.0.1:9999", virtual_key: null, error: null,
    model_test_uses_running_gateway: false, ...overrides,
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

  it("refreshes only the cached startup Agent set", async () => {
    await getCachedAgentViews();
    expect(invokeMock).toHaveBeenCalledWith("get_cached_agent_views");
  });

  it("waits for one reachable proxy generation before connecting an Agent", async () => {
    await ensureServeRunning();
    expect(invokeMock).toHaveBeenCalledWith("ensure_serve_running");
  });

  it("applies an exact direct target without exposing display order", async () => {
    await setDirectRoute("openai", "gpt-5.6", "codex");
    expect(invokeMock).toHaveBeenCalledWith("set_direct_route", {
      upstream: "openai",
      model: "gpt-5.6",
      agentId: "codex",
    });
  });

  it("sets the direct routing mode through the existing mode command", async () => {
    await setRoutingMode("direct");
    expect(invokeMock).toHaveBeenCalledWith("set_routing_mode", {
      mode: "direct",
      agentId: null,
    });
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
      "discard an unconsumed pending plan",
      () => discardAgentPlan("operation", "confirmation"),
      "discard_agent_plan",
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
      "get read-only drift facts",
      () => getAgentDrift("claude-code", "/opt/claude"),
      "get_agent_drift",
      { agentId: "claude-code", installationPath: "/opt/claude" },
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
  it("maps display-only Agent budget commands to named IPC fields", async () => {
    await getAgentBudgets();
    expect(invokeMock).toHaveBeenLastCalledWith("get_agent_budgets");

    await setAgentBudget("codex", 2_500_000, 80, 1_700_000_000_000, 1_800_000_000_000, 7);
    expect(invokeMock).toHaveBeenLastCalledWith("set_agent_budget", {
      agentId: "codex",
      limitMicros: 2_500_000,
      warningPercent: 80,
      periodStartMs: 1_700_000_000_000,
      periodEndMs: 1_800_000_000_000,
      expiryWarningDays: 7,
    });

    await removeAgentBudget("codex");
    expect(invokeMock).toHaveBeenLastCalledWith("remove_agent_budget", { agentId: "codex" });
  });

  it("maps versioned model price edits with optimistic concurrency", async () => {
    await getPriceTable();
    expect(invokeMock).toHaveBeenLastCalledWith("get_price_table");

    await setModelPrice("gpt-5", {
      input_per_mtok: 1_000_000,
      output_per_mtok: 2_000_000,
      cache_read_per_mtok: 300_000,
      cache_write_per_mtok: 4_000_000,
      reasoning_per_mtok: null,
    }, 7);
    expect(invokeMock).toHaveBeenLastCalledWith("set_model_price", {
      model: "gpt-5",
      inputPerMtok: 1_000_000,
      outputPerMtok: 2_000_000,
      cacheReadPerMtok: 300_000,
      cacheWritePerMtok: 4_000_000,
      reasoningPerMtok: null,
      expectedVersion: 7,
    });

    await removeModelPrice("gpt-5", 8);
    expect(invokeMock).toHaveBeenLastCalledWith("remove_model_price", {
      model: "gpt-5",
      expectedVersion: 8,
    });
  });

  it("requests a read-only public price suggestion without saving it", async () => {
    await suggestModelPrice(null, "gpt-5");
    expect(invokeMock).toHaveBeenLastCalledWith("suggest_model_price", {
      providerId: null,
      modelId: "gpt-5",
    });
  });

  it("subscribes to the stable proxy lifecycle event", async () => {
    const handler = vi.fn();
    await listenServeState(handler);
    expect(listenMock).toHaveBeenCalledWith("serve-state-changed", expect.any(Function));
  });

  it("subscribes to desktop updater progress without exposing updater permissions", async () => {
    const handler = vi.fn();
    await listenDesktopUpdateProgress(handler);
    expect(listenMock).toHaveBeenCalledWith("desktop-update-progress", expect.any(Function));
  });

  it("listens before starting a model stream, filters request IDs, and always unlistens", async () => {
    const onDelta = vi.fn();
    const unlisten = vi.fn();
    let streamHandler: ((event: { payload: { request_id: string; delta: string; first_token_ms: number | null } }) => void) | undefined;
    listenMock.mockImplementation(async (_event, handler) => {
      streamHandler = handler as typeof streamHandler;
      return unlisten;
    });
    invokeMock.mockImplementation(async (command) => {
      if (command === "test_model_chat_stream") {
        streamHandler?.({ payload: { request_id: "other", delta: "ignore", first_token_ms: 4 } });
        streamHandler?.({ payload: { request_id: "model-test-1", delta: "hello", first_token_ms: 8 } });
        return { content: "hello", first_token_ms: 8, latency_ms: 20 };
      }
      return undefined;
    });

    const reply = await testModelChatStream(
      [{ role: "user", content: "hello" }],
      "model-test-1",
      onDelta,
    );

    expect(listenMock).toHaveBeenCalledWith("model-test-stream", expect.any(Function));
    expect(invokeMock).toHaveBeenCalledWith("test_model_chat_stream", {
      messages: [{ role: "user", content: "hello" }],
      requestId: "model-test-1",
    });
    expect(onDelta).toHaveBeenCalledOnce();
    expect(onDelta).toHaveBeenCalledWith({ request_id: "model-test-1", delta: "hello", first_token_ms: 8 });
    expect(reply).toEqual({ content: "hello", first_token_ms: 8, latency_ms: 20 });
    expect(unlisten).toHaveBeenCalledOnce();
  });

  it.each([
    ["get state", () => getState(), "get_state", undefined],
    ["get runtime facts", () => getRuntimeState(), "get_runtime_state", undefined],
    ["get Agent backup directory", () => getAgentBackupDirectory(), "get_agent_backup_directory", undefined],
    ["open Agent backup directory", () => openAgentBackupDirectory(), "open_agent_backup_directory", undefined],
    ["add provider", () => addProvider("p", "https://p/v1", ["m"], "k"), "add_provider_with_credential", {
      name: "p",
      baseUrl: "https://p/v1",
      models: ["m"],
      apiKey: "k",
      local: false,
      credentialSource: "store",
      credentialReference: null,
      providerDialect: "openai-compatible",
    }],
    ["add managed enterprise route", () => addManagedEnterpriseRoute("https://enterprise.example.com/v1", "k", "enterprise-reasoner"), "add_managed_enterprise_route", {
      baseUrl: "https://enterprise.example.com/v1",
      apiKey: "k",
      model: "enterprise-reasoner",
    }],
    ["list free provider presets", () => listFreeProviderPresets(), "list_free_provider_presets", undefined],
    ["list public provider models", () => listPublicProviderModels(["openai", "qwen"]), "list_public_provider_models", { providerIds: ["openai", "qwen"] }],
    ["validate and add free provider", () => addFreeProvider("nvidia", ["openai/gpt-oss-120b"], "nvapi", true), "add_free_provider", { presetId: "nvidia", selectedModels: ["openai/gpt-oss-120b"], apiKey: "nvapi", guardConfirmed: true }],
    ["add local provider", () => addProvider("ollama", "http://127.0.0.1:11434/v1", ["m"], null, true), "add_provider_with_credential", {
      name: "ollama",
      baseUrl: "http://127.0.0.1:11434/v1",
      models: ["m"],
      apiKey: null,
      local: true,
      credentialSource: "none",
      credentialReference: null,
      providerDialect: "openai-compatible",
    }],
    ["add Azure OpenAI v1 provider", () => addProvider(
      "azure",
      "https://fixture.openai.azure.com/openai/v1",
      ["deployment-fixture"],
      "k",
      false,
      "store",
      null,
      "azure-openai-v1",
    ), "add_provider_with_credential", {
      name: "azure",
      baseUrl: "https://fixture.openai.azure.com/openai/v1",
      models: ["deployment-fixture"],
      apiKey: "k",
      local: false,
      credentialSource: "store",
      credentialReference: null,
      providerDialect: "azure-openai-v1",
    }],
    ["set local routing on", () => setLocalRouting(true, false), "set_local_routing", { localOnly: true, allowCloudFallback: false }],
    ["set local routing off", () => setLocalRouting(false, false), "set_local_routing", { localOnly: false, allowCloudFallback: false }],
    ["edit provider", () => editProvider("p", "https://p/v1", null), "edit_provider", { name: "p", baseUrl: "https://p/v1", apiKey: null }],
    ["edit provider with credential", () => editProvider(
      "p",
      "https://p/v1",
      null,
      "env",
      "P_KEY",
    ), "edit_provider_with_credential", {
      name: "p",
      baseUrl: "https://p/v1",
      apiKey: null,
      credentialSource: "env",
      credentialReference: "P_KEY",
    }],
    ["preview provider removal", () => previewProviderRemoval("p"), "preview_provider_removal", { name: "p" }],
    ["remove provider", () => removeProvider("p"), "remove_provider", { name: "p" }],
    ["restore provider", () => restoreProvider("p"), "restore_provider", { name: "p" }],
    ["purge deleted providers", () => purgeDeletedProviders(), "purge_deleted_providers", undefined],
    ["discover models", () => discoverProviderModels("p", "https://p/v1", null), "discover_provider_models", { name: "p", baseUrl: "https://p/v1", apiKey: null }],
    ["verify enterprise route", () => verifyEnterpriseRoute("enterprise", "https://p/v1", "k"), "verify_enterprise_route", { name: "enterprise", baseUrl: "https://p/v1", apiKey: "k" }],
    ["test provider", () => testProvider("p"), "test_provider", { name: "p" }],
    ["cancel model test chat", () => cancelModelTestChat("model-test-1"), "cancel_model_test_chat", { requestId: "model-test-1" }],
    ["declare model vision", () => setProviderModelVision("p", "m", true), "set_provider_model_vision", { name: "p", model: "m", supported: true }],
    ["set model limits", () => setProviderModelLimits("p", "m", 128000, 32768), "set_provider_model_limits", { name: "p", model: "m", contextWindow: 128000, maxOutputTokens: 32768 }],
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
    ["settings", () => setSettings(false, true, {
      egress_mode: "http",
      egress_proxy_url: "http://proxy.internal:8080",
      egress_no_proxy: ["localhost"],
      egress_auth_username: "x",
      egress_auth_slot: "proxy_password",
    }), "set_settings", {
      auth: false,
      metrics: true,
      egressMode: "http",
      egressProxyUrl: "http://proxy.internal:8080",
      egressNoProxy: ["localhost"],
      egressAuthUsername: "x",
      egressAuthSlot: "proxy_password",
    }],
    ["desktop update check", () => checkDesktopUpdate(), "check_desktop_update", undefined],
    ["desktop update install", () => installDesktopUpdateAndRestart("1.1.3"), "install_desktop_update_and_restart", { expectedVersion: "1.1.3" }],
    ["recovery state", () => getRecoveryState(), "get_recovery_state", undefined],
    ["recovery diagnostics", () => getRecoveryDiagnostics(), "get_recovery_diagnostics", undefined],
    ["recovery folder", () => openRecoveryFolder(), "open_recovery_folder", undefined],
    ["confirmed recovery export", () => exportRecoveryBundle(true), "export_recovery_bundle", { confirmed: true }],
    ["frontend diagnostic", () => recordFrontendDiagnostic({ kind: "window_error", message: "boom", stack: null, component_stack: null }), "record_frontend_diagnostic", {
      event: { kind: "window_error", message: "boom", stack: null, component_stack: null },
    }],
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
          : url.includes("egress")
            ? { mode: "direct", proxy_url: null, no_proxy: [], auth_slot: null, routes: [], fixed_direct_classes: ["update_check"] }
          : { dir: "/plugins", agent: "a", dialects: [], listing: "" };
      return new Response(JSON.stringify(body), { status: 200 });
    });
    setAdminEndpoint(serveFixture({ phase: "running", app_runtime: "running", listener_reachable: true, virtual_key: "virtual" }));

    await getStats("24h", "model", "codex", "openai-responses");
    const receipts = await getRecentReceipts(5);
    await getRouterTable();
    await getPlugins();
    await getEgress();

    expect(receipts[0]?.request_id).toBe("receipt-1");
    expect(fetchMock).toHaveBeenCalledTimes(5);
    expect(fetchMock.mock.calls[0]?.[0]).toBe("http://127.0.0.1:9999/admin/stats?since=24h&by=model&agent=codex&source=openai-responses");
    expect(fetchMock.mock.calls[0]?.[1]).toEqual({ headers: { authorization: "Bearer virtual" } });
    expect(fetchMock.mock.calls[1]?.[0]).toBe("http://127.0.0.1:9999/admin/receipts");
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it("refreshes plugin eligibility through IPC in the running Tauri shell", async () => {
    Object.defineProperty(window, "__TAURI_INTERNALS__", {
      configurable: true,
      value: {},
    });
    vi.resetModules();
    const tauriApi = await import("./api");
    const fetchMock = vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(JSON.stringify({ dir: "/runtime-plugins", agent: "a", dialects: [], listing: "" })),
    );
    invokeMock.mockResolvedValue({
      dir: "/draft-plugins",
      agent: "a",
      dialects: ["openai-compatible"],
      listing: "provider-openai-compatible",
    });
    tauriApi.setAdminEndpoint(serveFixture({
      phase: "running",
      app_runtime: "running",
      listener_reachable: true,
      virtual_key: "virtual",
    }));

    let plugins: Awaited<ReturnType<typeof tauriApi.getPlugins>> | null = null;
    try {
      plugins = await tauriApi.getPlugins();
    } finally {
      delete (window as typeof window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
      vi.resetModules();
    }

    expect(plugins?.dir).toBe("/draft-plugins");
    expect(invokeMock).toHaveBeenCalledWith("get_plugins");
    expect(fetchMock).not.toHaveBeenCalled();
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
