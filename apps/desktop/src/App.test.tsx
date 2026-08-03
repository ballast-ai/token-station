import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import App, { configSaveStatus } from "./App";
import { getStats } from "./api";
import type { AgentRouteView, AgentUiMetadataView, AgentView, ServeView, StateView } from "./api";
import { AGENT_VISIBILITY_STORAGE_KEY } from "./components/AgentVisibilityPreferences";
import { LANGUAGE_STORAGE_KEY } from "./components/LanguageProvider";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("./api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./api")>();
  return { ...actual, getStats: vi.fn() };
});
const invokeMock = vi.mocked(invoke);
const listenMock = vi.mocked(listen);
const getStatsMock = vi.mocked(getStats);

const emptyRoute: AgentRouteView = {
  mode: "inherit",
  tiers: {
    high: { upstream: null, model: null },
    mid: { upstream: null, model: null },
    low: { upstream: null, model: null },
  },
  config_error: null,
  profile: null,
  routing_mode: "tiered",
};

const registryFixture: AgentUiMetadataView[] = [
  { agent_id: "claude-code", legacy_kind: "cc", display_name: "Claude Code", icon_key: "claude", admission: "supported", ui_order: 10, nav_mark: "C" },
  { agent_id: "claude-desktop", legacy_kind: null, display_name: "Claude Desktop", icon_key: "claude-desktop", admission: "supported", ui_order: 15, nav_mark: "CD" },
  { agent_id: "codex", legacy_kind: "codex", display_name: "Codex", icon_key: "codex", admission: "supported", ui_order: 20, nav_mark: "X" },
  { agent_id: "gemini-cli", legacy_kind: null, display_name: "Gemini CLI", icon_key: "gemini", admission: "supported", ui_order: 30, nav_mark: "G" },
  { agent_id: "opencode", legacy_kind: "opencode", display_name: "OpenCode", icon_key: "opencode", admission: "supported", ui_order: 40, nav_mark: "O" },
  { agent_id: "openclaw", legacy_kind: null, display_name: "OpenClaw", icon_key: "openclaw", admission: "supported", ui_order: 50, nav_mark: "OC" },
  { agent_id: "nous-hermes-agent", legacy_kind: null, display_name: "Hermes Agent", icon_key: "hermes", admission: "supported", ui_order: 60, nav_mark: "H" },
  { agent_id: "future-agent", legacy_kind: null, display_name: "Future Agent", icon_key: "future", admission: "discovery_only" },
];
const supportedRegistryFixture = registryFixture.filter(
  (metadata) => metadata.admission === "supported",
);
const agentIds = supportedRegistryFixture.map((metadata) => metadata.agent_id);
const agentDisplayNames = supportedRegistryFixture.map(
  (metadata) => metadata.display_name,
);
const agentNavigationNames = agentDisplayNames;

const statsFixture = {
  total: {
    requests: 12,
    errors: 1,
    p50_latency_ms: 48,
    p95_latency_ms: 320,
    input_tokens: 2400,
    output_tokens: 800,
    cache_read_tokens: 0,
    cache_write_tokens: 0,
    reasoning_tokens: 0,
    cost_micros: null,
    priced_requests: 0,
    unpriced_requests: 12,
  },
  groups: [],
  by: null,
  empty: false,
};

const registryWithVirtualSupportedAgent: AgentUiMetadataView[] = [
  ...registryFixture,
  {
    agent_id: "virtual-agent",
    legacy_kind: null,
    display_name: "Virtual Agent",
    icon_key: "virtual",
    admission: "supported",
    ui_order: 999,
    nav_mark: "V",
  },
];

function serveFixture(overrides: Partial<ServeView> = {}): ServeView {
  return {
    phase: "stopped",
    app_runtime: "stopped",
    listener_reachable: false,
    agent_connected: false,
    running_revision: null,
    instance_id: null,
    listen: "127.0.0.1:8787",
    virtual_key: null,
    error: null,
    ...overrides,
  };
}

function stateFixture(overrides: Partial<StateView> = {}): StateView {
  return {
    providers: [],
    tiers: {
      high: { upstream: null, model: null },
      mid: { upstream: null, model: null },
      low: { upstream: null, model: null },
    },
    keywords: { high: [], mid: [], low: [] },
    agent_routes: Object.fromEntries(agentIds.map((id) => [id, structuredClone(emptyRoute)])),
    profiles: [],
    local_only: false,
    allow_cloud_fallback: false,
    routing_mode: "tiered",
    quota_accounts: [],
    serve: serveFixture(),
    draft_revision: 0,
    saved_revision: 0,
    config_dirty: false,
    config_error: null,
    settings: {
      listen: "127.0.0.1:8787",
      auth: true,
      metrics: true,
      data_dir: "/tmp/token-station-test/data",
      plugins_dir: "/tmp/token-station-test/plugins",
      agent: "test-agent",
      version: "test-version",
      egress_mode: "direct",
      egress_proxy_url: "",
      egress_no_proxy: [],
      egress_auth_username: "",
      egress_auth_slot: "",
    },
    ...overrides,
  };
}

const scannedClaude: AgentView = {
  metadata: registryFixture[0],
  installations: [{
    managed: false,
    connected: false,
    adapter_ready: true,
    discovery: {
      agent_id: "claude-code",
      executable_path: "/opt/claude",
      canonical_path: "/opt/claude",
      binary_source: "path",
      modified_at_ms: null,
      binary_sha256: null,
      upgrade_command: null,
      version_raw: "9.9.9",
      version_normalized: "9.9.9",
      environment: "macos",
      evidence: [{ source: "path", observed_path: "/opt/claude", is_path_default: true }],
      is_path_default: true,
      runnable: true,
      config_candidates: ["/tmp/settings.json"],
      config_fingerprint: null,
      conflict_group: null,
      diagnostics: [],
      scanned_at_ms: 1,
    },
    compatibility: {
      agent_id: "claude-code",
      installation_path: "/opt/claude",
      status: "DETECTED_VERIFIED",
      reason_code: "DefaultAdmission",
      message: "已通过只读预检，可以安全接入",
      matched_catalog_version: "fixture",
      connector_id: "claude-code-v1",
      allowed_actions: ["preview_connect"],
    },
  }],
  status: "DETECTED_VERIFIED",
  catalog_sequence: 1,
  catalog_expires_at_ms: null,
  catalog_source: "builtin",
  catalog_warning: null,
};

function defaultAdmittedClaude(): AgentView {
  const value = structuredClone(scannedClaude);
  value.installations[0].discovery.version_raw = "2.1.210";
  value.installations[0].discovery.version_normalized = "2.1.210";
  value.installations[0].compatibility.reason_code = "DefaultAdmission";
  value.installations[0].compatibility.message = "已通过只读预检，可以安全接入";
  value.installations[0].compatibility.allowed_actions = ["preview_connect"];
  return value;
}

function projectionPlan(
  operationId: string,
  confirmationToken: string,
  intent: "connect" | "disconnect" = "connect",
) {
  return {
    operation_id: operationId,
    confirmation_token: confirmationToken,
    intent,
    target_config_path: "/tmp/settings.json",
    related_config_paths: [],
    human_diff: "~ /env/ANTHROPIC_BASE_URL: <设置受管值>",
    changes: [{
      operation: "replace",
      path: { segments: ["env", "ANTHROPIC_BASE_URL"] },
      sensitive: false,
      summary: "<设置受管值>",
    }],
  };
}

function navigation() {
  return within(screen.getByRole("navigation", { name: /主导航|Main navigation/ }));
}

async function openRouting(user: ReturnType<typeof userEvent.setup>) {
  await user.click((await screen.findByRole("navigation", { name: /主导航|Main navigation/ })).querySelector<HTMLButtonElement>('button[aria-label="路由"], button[aria-label="Routing"]')!);
  await screen.findByRole("heading", { name: /全局路由|Global routing/ });
}

async function openAgents(user: ReturnType<typeof userEvent.setup>) {
  await user.click((await screen.findByRole("navigation", { name: /主导航|Main navigation/ })).querySelector<HTMLButtonElement>('button[aria-label="Agent"], button[aria-label="Agents"]')!);
  await screen.findByRole("heading", { name: /Agent 管理|Agents/ });
}

async function openAgent(user: ReturnType<typeof userEvent.setup>, name: string) {
  await openAgents(user);
  await user.click(screen.getByRole("button", { name }));
  await screen.findByRole("heading", { name });
}

async function openAgentVisibility(user: ReturnType<typeof userEvent.setup>) {
  await user.click(await screen.findByRole("button", { name: "设置" }));
  await user.click(screen.getByRole("button", { name: /Agent 显示/ }));
  await screen.findByRole("heading", { name: "Agent 显示" });
}

beforeEach(() => {
  window.localStorage.setItem(LANGUAGE_STORAGE_KEY, "zh-CN");
  window.localStorage.removeItem(AGENT_VISIBILITY_STORAGE_KEY);
  listenMock.mockReset();
  listenMock.mockResolvedValue(vi.fn());
  getStatsMock.mockReset();
  getStatsMock.mockResolvedValue(statsFixture);
  const initial = stateFixture();
  invokeMock.mockImplementation(async (command) => {
    if (command === "get_state") return initial;
    if (command === "list_agent_registry") return registryFixture;
    if (command === "scan_agents") return [];
    if (command === "get_request_receipts") return { items: [], total: 0, page: 1, page_size: 20 };
    throw new Error(`unexpected IPC command: ${command}`);
  });
});

it("renders a supported virtual Agent entirely from registry metadata", async () => {
  const user = userEvent.setup();
  invokeMock.mockImplementation(async (command) => {
    if (command === "get_state") return stateFixture();
    if (command === "list_agent_registry") return registryWithVirtualSupportedAgent;
    if (command === "scan_agents") return [];
    throw new Error(`unexpected IPC command: ${command}`);
  });

  render(<App />);
  await openAgents(user);
  expect(screen.getByRole("button", { name: "Virtual Agent" })).toHaveAttribute("title", "Virtual Agent · 未检测");
  expect(screen.getByText("V")).toBeInTheDocument();
});

describe("desktop station navigation", () => {
  it("opens on Overview and exposes the six primary desktop destinations", async () => {
    render(<App />);

    expect(await screen.findByRole("heading", { name: "概览" })).toBeInTheDocument();
    const nav = within(screen.getByLabelText("主导航"));
    for (const name of ["概览", "路由", "Agent", "供应商", "用量", "设置"]) {
      expect(nav.getByRole("button", { name })).toBeInTheDocument();
    }
    expect(nav.queryByRole("button", { name: "日志" })).toBeNull();
    const revisionChain = screen.getByTestId("revision-chain");
    expect(revisionChain).toHaveAccessibleName("已保存 revision 0；待应用；未运行");
    expect(screen.getByLabelText("系统摘要")).not.toContainElement(revisionChain);
    expect(screen.getByRole("region", { name: "当前路由快照" })).toContainElement(revisionChain);
    expect(await screen.findByText(/成功率 91\.7% · P95 320ms/)).toBeInTheDocument();
    expect(getStatsMock).toHaveBeenCalledWith("24h", null);
  });

  it("shows request cost on Overview without duplicating the Agent rescan action", async () => {
    getStatsMock.mockResolvedValueOnce({
      ...statsFixture,
      total: {
        ...statsFixture.total,
        cost_micros: 2_340_000,
        priced_requests: 12,
        unpriced_requests: 0,
      },
    });

    render(<App />);

    expect(await screen.findByText("今日请求")).toBeInTheDocument();
    expect(await screen.findByText("$2.34")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "重新扫描 Agent" })).toBeNull();
    expect(screen.getByRole("button", { name: /启动代理.*127\.0\.0\.1:8787/ })).toBeInTheDocument();
  });

  it("opens request logs as a secondary view inside Usage", async () => {
    const user = userEvent.setup();
    render(<App />);

    await openRouting(user);
    await openAgents(user);
    await user.click(navigation().getByRole("button", { name: "供应商" }));
    expect(await screen.findByRole("heading", { name: "供应商管理" })).toBeInTheDocument();
    await user.click(navigation().getByRole("button", { name: "用量" }));
    const usageNavigation = await screen.findByRole("tablist", { name: "用量视图" });
    await user.click(within(usageNavigation).getByRole("tab", { name: "请求日志" }));
    expect(await screen.findByRole("heading", { name: "请求日志", level: 1 })).toBeInTheDocument();
    expect(await screen.findByText("当前筛选范围没有请求日志。")).toBeInTheDocument();
    expect(navigation().getByRole("button", { name: "用量" })).toHaveAttribute("aria-current", "page");
  });

  it("keeps the Agent list visible while editing the selected Agent on the right", async () => {
    const user = userEvent.setup();
    render(<App />);

    await openAgents(user);
    const agentNavigation = screen.getByRole("navigation", { name: "Agent 列表" });
    const claudeCodeButton = within(agentNavigation).getByRole("button", { name: "Claude Code" });
    expect(claudeCodeButton).toHaveAttribute("aria-current", "page");
    expect(claudeCodeButton.querySelector("svg")).toBeInTheDocument();
    expect(claudeCodeButton.querySelector('[style*="background"]')).toBeNull();
    expect(screen.getByRole("heading", { name: "Claude Code", level: 2 })).toBeInTheDocument();

    await user.click(within(agentNavigation).getByRole("button", { name: "Codex" }));
    expect(await screen.findByRole("heading", { name: "Codex", level: 2 })).toBeInTheDocument();
    expect(within(agentNavigation).getByRole("button", { name: "Codex" })).toHaveAttribute("aria-current", "page");
  });

  it("changes the selected Agent routing strategy from the detail workspace", async () => {
    const user = userEvent.setup();
    const initial = stateFixture();
    invokeMock.mockImplementation(async (command) => {
      if (command === "get_state") return initial;
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return [];
      if (command === "set_routing_mode") return {
        ...initial,
        agent_routes: {
          ...initial.agent_routes,
          "claude-code": { ...initial.agent_routes["claude-code"], routing_mode: "quota_first" },
        },
      };
      throw new Error(`unexpected IPC command: ${command}`);
    });

    render(<App />);
    await openAgents(user);
    const strategyTabs = screen.getByRole("tablist", { name: "Agent 路由策略" });
    await user.click(within(strategyTabs).getByRole("tab", { name: "额度优先" }));

    expect(invokeMock).toHaveBeenCalledWith("set_routing_mode", { mode: "quota_first", agentId: "claude-code" });
    expect(within(strategyTabs).getByRole("tab", { name: "额度优先" })).toHaveAttribute("aria-selected", "true");
  });

  it("puts the real global routing-mode switch below the routing title", async () => {
    const user = userEvent.setup();
    const initial = stateFixture();
    invokeMock.mockImplementation(async (command) => {
      if (command === "get_state") return initial;
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return [];
      if (command === "set_routing_mode") return { ...initial, routing_mode: "quota_first" };
      throw new Error(`unexpected IPC command: ${command}`);
    });

    render(<App />);
    expect(screen.queryByRole("tablist", { name: "路由模式" })).toBeNull();
    await openRouting(user);
    const modeTabs = screen.getByRole("tablist", { name: "路由模式" });
    await user.click(within(modeTabs).getByRole("tab", { name: "额度优先" }));

    expect(invokeMock).toHaveBeenCalledWith("set_routing_mode", { mode: "quota_first", agentId: null });
    expect(within(modeTabs).getByRole("tab", { name: "额度优先" })).toHaveAttribute("aria-selected", "true");
  });

  it("maps the four revision relationships to stable save copy", () => {
    expect(configSaveStatus(stateFixture(), "zh-CN")).toBe("无改动");
    expect(configSaveStatus(stateFixture({ config_dirty: true, draft_revision: 2, saved_revision: 1 }), "zh-CN")).toBe("有未保存更改");
    expect(configSaveStatus(stateFixture({
      saved_revision: 2,
      serve: serveFixture({ app_runtime: "running", listener_reachable: true, running_revision: 1 }),
    }), "zh-CN")).toBe("已保存尚未应用");
    expect(configSaveStatus(stateFixture({
      saved_revision: 2,
      serve: serveFixture({ app_runtime: "running", listener_reachable: true, running_revision: 2 }),
    }), "zh-CN")).toBe("运行中 revision 2");
  });

  it("shows the revision save state and makes 保存并应用 start a real apply", async () => {
    const user = userEvent.setup();
    const dirty = stateFixture({ draft_revision: 2, saved_revision: 1, config_dirty: true });
    const applying = stateFixture({
      draft_revision: 2,
      saved_revision: 2,
      config_dirty: false,
      serve: serveFixture({ phase: "starting" }),
    });
    invokeMock.mockImplementation(async (command) => {
      if (command === "get_state") return dirty;
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return [];
      if (command === "serve_start") return applying;
      throw new Error(`unexpected IPC command: ${command}`);
    });

    render(<App />);
    await openRouting(user);
    expect(await screen.findByTestId("config-save-status")).toHaveTextContent("有未保存更改");
    await user.click(screen.getByRole("button", { name: "保存并应用" }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("serve_start"));
    expect(invokeMock).not.toHaveBeenCalledWith("save_config");
  });

  it.each([
    [
      "the old running revision",
      serveFixture({
        phase: "running",
        app_runtime: "running",
        listener_reachable: true,
        running_revision: 1,
      }),
      null,
    ],
    [
      "an apply error",
      serveFixture({
        phase: "running",
        app_runtime: "running",
        listener_reachable: true,
        running_revision: 2,
        error: "已保存尚未应用：gateway_init: fixture failure",
      }),
      "已保存尚未应用：gateway_init: fixture failure",
    ],
  ])("does not report configuration applied when starting returns to %s", async (_case, terminal, expectedError) => {
    const user = userEvent.setup();
    let emitServe: ((serve: ServeView) => void) | undefined;
    listenMock.mockImplementation(async (_eventName, handler) => {
      emitServe = (serve) => handler({ payload: serve } as Parameters<typeof handler>[0]);
      return () => undefined;
    });
    const dirty = stateFixture({ draft_revision: 2, saved_revision: 1, config_dirty: true });
    const applying = stateFixture({
      draft_revision: 2,
      saved_revision: 2,
      config_dirty: false,
      serve: serveFixture({ phase: "starting" }),
    });
    invokeMock.mockImplementation(async (command) => {
      if (command === "get_state") return dirty;
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return [];
      if (command === "serve_start") return applying;
      throw new Error(`unexpected IPC command: ${command}`);
    });

    render(<App />);
    await openRouting(user);
    await user.click(await screen.findByRole("button", { name: "保存并应用" }));
    expect(await screen.findByText("正在应用配置…")).toBeInTheDocument();
    act(() => emitServe?.(terminal));

    await waitFor(() => expect(screen.queryByText("正在应用配置…")).toBeNull());
    expect(screen.queryByText(/配置已应用/)).toBeNull();
    if (expectedError) {
      expect(screen.getByText(expectedError)).toBeInTheDocument();
    }
  });

  it("reports configuration applied only when the requested revision is running without an error", async () => {
    const user = userEvent.setup();
    let emitServe: ((serve: ServeView) => void) | undefined;
    listenMock.mockImplementation(async (_eventName, handler) => {
      emitServe = (serve) => handler({ payload: serve } as Parameters<typeof handler>[0]);
      return () => undefined;
    });
    const dirty = stateFixture({ draft_revision: 2, saved_revision: 1, config_dirty: true });
    const applying = stateFixture({
      draft_revision: 2,
      saved_revision: 2,
      config_dirty: false,
      serve: serveFixture({ phase: "starting" }),
    });
    invokeMock.mockImplementation(async (command) => {
      if (command === "get_state") return dirty;
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return [];
      if (command === "serve_start") return applying;
      throw new Error(`unexpected IPC command: ${command}`);
    });

    render(<App />);
    await openRouting(user);
    await user.click(await screen.findByRole("button", { name: "保存并应用" }));
    expect(await screen.findByText("正在应用配置…")).toBeInTheDocument();
    act(() => emitServe?.(serveFixture({
      phase: "running",
      app_runtime: "running",
      listener_reachable: true,
      running_revision: 2,
    })));

    expect(await screen.findByText("配置已应用 · revision 2")).toBeInTheDocument();
  });

  it("keeps waiting when a stale runtime poll arrives before the requested revision", async () => {
    const user = userEvent.setup();
    let emitServe: ((serve: ServeView) => void) | undefined;
    listenMock.mockImplementation(async (_eventName, handler) => {
      emitServe = (serve) => handler({ payload: serve } as Parameters<typeof handler>[0]);
      return () => undefined;
    });
    const dirty = stateFixture({ draft_revision: 2, saved_revision: 1, config_dirty: true });
    const applying = stateFixture({
      draft_revision: 2,
      saved_revision: 2,
      config_dirty: false,
      serve: serveFixture({ phase: "starting" }),
    });
    invokeMock.mockImplementation(async (command) => {
      if (command === "get_state") return dirty;
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return [];
      if (command === "serve_start") return applying;
      throw new Error(`unexpected IPC command: ${command}`);
    });

    render(<App />);
    await openRouting(user);
    await user.click(await screen.findByRole("button", { name: "保存并应用" }));
    expect(await screen.findByText("正在应用配置…")).toBeInTheDocument();
    act(() => emitServe?.(serveFixture({
      phase: "running",
      app_runtime: "running",
      listener_reachable: true,
      running_revision: 1,
    })));
    await waitFor(() => expect(screen.queryByText("正在应用配置…")).toBeNull());
    expect(screen.queryByText(/配置已应用/)).toBeNull();

    act(() => emitServe?.(serveFixture({
      phase: "running",
      app_runtime: "running",
      listener_reachable: true,
      running_revision: 2,
    })));
    expect(await screen.findByText("配置已应用 · revision 2")).toBeInTheDocument();
  });

  it("does not treat an ordinary first proxy startup as a completed configuration apply", async () => {
    let emitServe: ((serve: ServeView) => void) | undefined;
    listenMock.mockImplementation(async (_eventName, handler) => {
      emitServe = (serve) => handler({ payload: serve } as Parameters<typeof handler>[0]);
      return () => undefined;
    });
    invokeMock.mockImplementation(async (command) => {
      if (command === "get_state") {
        return stateFixture({
          saved_revision: 1,
          serve: serveFixture({ phase: "starting" }),
        });
      }
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return [];
      throw new Error(`unexpected IPC command: ${command}`);
    });

    render(<App />);
    expect(await screen.findByText("正在应用配置…")).toBeInTheDocument();
    act(() => emitServe?.(serveFixture({
      phase: "running",
      app_runtime: "running",
      listener_reachable: true,
      running_revision: 1,
    })));

    await waitFor(() => expect(screen.queryByText("正在应用配置…")).toBeNull());
    expect(screen.queryByText(/配置已应用/)).toBeNull();
  });

  it("refreshes the independent Agent runtime fact within the 500ms poll", async () => {
    const connected = stateFixture({
      serve: serveFixture({
        phase: "running",
        app_runtime: "running",
        listener_reachable: true,
        agent_connected: true,
        running_revision: 1,
        instance_id: "runtime-a",
      }),
    });
    invokeMock.mockImplementation(async (command) => {
      if (command === "get_state") return connected;
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return [];
      if (command === "get_runtime_state") {
        return serveFixture({
          phase: "running",
          app_runtime: "running",
          listener_reachable: true,
          agent_connected: false,
          running_revision: 1,
          instance_id: "runtime-a",
        });
      }
      throw new Error(`unexpected IPC command: ${command}`);
    });

    render(<App />);
    expect(await screen.findByTestId("agent-runtime-connection")).toHaveTextContent("Agent：已连接");
    await waitFor(
      () => expect(screen.getByTestId("agent-runtime-connection")).toHaveTextContent("Agent：未连接"),
      { timeout: 1_500 },
    );
  });

  it("rescans once the runtime becomes ready after a not-ready first load", async () => {
    // The gateway can be unavailable when the app opens. The initial load scan has a not-ready runtime state. Managed Agents
    // This can incorrectly show Repair required. When runtime becomes ready, scan again to align the card with real state.
    const notReady = stateFixture({
      serve: serveFixture({
        phase: "starting",
        app_runtime: "stopped",
        listener_reachable: false,
        running_revision: 1,
        instance_id: "runtime-a",
      }),
    });
    invokeMock.mockImplementation(async (command) => {
      if (command === "get_state") return notReady;
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return [];
      if (command === "get_runtime_state") {
        return serveFixture({
          phase: "running",
          app_runtime: "running",
          listener_reachable: true,
          running_revision: 1,
          instance_id: "runtime-a",
        });
      }
      throw new Error(`unexpected IPC command: ${command}`);
    });

    render(<App />);
    // The first scan comes from load() while runtime is not ready.
    await waitFor(() => expect(invokeMock.mock.calls.filter(([command]) => command === "scan_agents")).toHaveLength(1));
    // The 500 ms poll marks runtime ready, and the transition triggers a rescan.
    await waitFor(
      () => expect(invokeMock.mock.calls.filter(([command]) => command === "scan_agents")).toHaveLength(2),
      { timeout: 1_500 },
    );
    // Runtime remains ready afterward, so no additional rescan occurs.
    await waitFor(() => expect(screen.getByTestId("agent-runtime-connection")).toBeInTheDocument());
    expect(invokeMock.mock.calls.filter(([command]) => command === "scan_agents")).toHaveLength(2);
  });

  it("rescans when a new serving instance replaces the old one without a readiness gap", async () => {
    const oldRuntime = stateFixture({
      serve: serveFixture({
        phase: "running",
        app_runtime: "running",
        listener_reachable: true,
        running_revision: 1,
        instance_id: "runtime-old",
      }),
    });
    invokeMock.mockImplementation(async (command) => {
      if (command === "get_state") return oldRuntime;
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return [];
      if (command === "get_runtime_state") {
        return serveFixture({
          phase: "running",
          app_runtime: "running",
          listener_reachable: true,
          running_revision: 2,
          instance_id: "runtime-new",
        });
      }
      throw new Error(`unexpected IPC command: ${command}`);
    });

    render(<App />);
    await waitFor(() => expect(
      invokeMock.mock.calls.filter(([command]) => command === "scan_agents"),
    ).toHaveLength(1));
    await waitFor(
      () => expect(
        invokeMock.mock.calls.filter(([command]) => command === "scan_agents"),
      ).toHaveLength(2),
      { timeout: 1_500 },
    );
    expect(invokeMock.mock.calls.filter(([command]) => command === "scan_agents")).toHaveLength(2);
  });

  it("renders every supported Registry Agent in ui_order, filters discovery-only entries, and scans only on load or explicit rescan", async () => {
    const user = userEvent.setup();
    render(<App />);
    await openAgents(user);

    const orderedButtons = agentNavigationNames.map((name) =>
      screen.getByRole("button", { name }));
    for (let index = 0; index < orderedButtons.length - 1; index += 1) {
      expect(
        orderedButtons[index].compareDocumentPosition(orderedButtons[index + 1])
        & Node.DOCUMENT_POSITION_FOLLOWING,
      ).toBeTruthy();
    }
    expect(screen.queryByRole("button", { name: /Future/i })).toBeNull();
    await waitFor(() => expect(invokeMock.mock.calls.filter(([command]) => command === "scan_agents")).toHaveLength(1));

    await user.click(screen.getByRole("button", { name: "Codex" }));
    expect(await screen.findByRole("heading", { name: "Codex" })).toBeInTheDocument();
    await user.click(navigation().getByRole("button", { name: "Agent" }));
    expect(invokeMock.mock.calls.filter(([command]) => command === "scan_agents")).toHaveLength(1);
    await user.click(screen.getByRole("button", { name: "重新扫描" }));
    await waitFor(() => expect(invokeMock.mock.calls.filter(([command]) => command === "scan_agents")).toHaveLength(2));
  });

  it("lets the user hide and restore a sidebar Agent from Settings without rescanning", async () => {
    const user = userEvent.setup();
    render(<App />);
    await screen.findByRole("heading", { name: "概览" });
    await waitFor(() => expect(invokeMock.mock.calls.filter(([command]) => command === "scan_agents")).toHaveLength(1));

    await openAgentVisibility(user);

    expect(screen.queryByText("AGENT NAVIGATION")).not.toBeInTheDocument();
    expect(screen.getByRole("group", { name: "Agent 显示选项" })).toBeInTheDocument();
    for (const name of agentDisplayNames) {
      expect(screen.getByRole("switch", { name, checked: true })).toBeInTheDocument();
    }
    expect(screen.getByRole("status")).toHaveTextContent("7 / 7 已显示");

    await user.click(screen.getByRole("switch", { name: "Codex", checked: true }));

    expect(screen.getByRole("switch", { name: "Codex", checked: false })).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent("6 / 7 已显示");
    expect(window.localStorage.getItem(AGENT_VISIBILITY_STORAGE_KEY)).toBe(
      JSON.stringify(["codex"]),
    );
    expect(invokeMock.mock.calls.filter(([command]) => command === "scan_agents")).toHaveLength(1);
    expect(invokeMock.mock.calls.filter(([command]) => [
      "set_settings",
      "serve_start",
      "serve_stop",
      "set_agent_route_mode",
      "plan_agent_connection",
      "plan_agent_disconnect",
      "apply_agent_plan",
    ].includes(String(command)))).toHaveLength(0);

    await user.click(screen.getByRole("switch", { name: "Codex", checked: false }));

    expect(screen.getByRole("switch", { name: "Codex", checked: true })).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent("7 / 7 已显示");
    expect(window.localStorage.getItem(AGENT_VISIBILITY_STORAGE_KEY)).toBe(
      JSON.stringify([]),
    );
    expect(invokeMock.mock.calls.filter(([command]) => command === "scan_agents")).toHaveLength(1);
  });

  it("restores hidden Agent preferences after remount while newly registered Agents remain visible", async () => {
    const user = userEvent.setup();
    const first = render(<App />);
    await openAgentVisibility(user);
    await user.click(screen.getByRole("switch", { name: "Codex", checked: true }));
    expect(window.localStorage.getItem(AGENT_VISIBILITY_STORAGE_KEY)).toBe(
      JSON.stringify(["codex"]),
    );
    first.unmount();

    invokeMock.mockImplementation(async (command) => {
      if (command === "get_state") return stateFixture();
      if (command === "list_agent_registry") return registryWithVirtualSupportedAgent;
      if (command === "scan_agents") return [];
      throw new Error(`unexpected IPC command: ${command}`);
    });
    render(<App />);
    await openAgents(user);
    expect(screen.queryByRole("button", { name: "Codex" })).toBeNull();
    expect(screen.getByRole("button", { name: "Virtual Agent" })).toBeInTheDocument();
    expect(window.localStorage.getItem(AGENT_VISIBILITY_STORAGE_KEY)).toBe(
      JSON.stringify(["codex"]),
    );
  });

  it("applies the stored preference before the first navigation render", async () => {
    const user = userEvent.setup();
    window.localStorage.setItem(
      AGENT_VISIBILITY_STORAGE_KEY,
      JSON.stringify(["codex"]),
    );
    const container = document.createElement("div");
    document.body.append(container);
    let codexWasAdded = false;
    const observer = new MutationObserver((records) => {
      for (const record of records) {
        for (const node of record.addedNodes) {
          if (!(node instanceof Element)) continue;
          const buttons = node.matches("button")
            ? [node]
            : [...node.querySelectorAll("button")];
          if (buttons.some((button) => button.textContent?.trim() === "Codex")) {
            codexWasAdded = true;
          }
        }
      }
    });
    observer.observe(container, { childList: true, subtree: true });

    render(<App />, { container });
    await openAgents(user);
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("scan_agents"));
    observer.disconnect();
    expect(screen.queryByRole("button", { name: "Codex" })).toBeNull();
    expect(screen.getByRole("button", { name: "Claude Code" })).toBeInTheDocument();
    expect(codexWasAdded).toBe(false);
  });

  it("falls back to showing every supported Agent when the stored preference is invalid", async () => {
    const user = userEvent.setup();
    window.localStorage.setItem(AGENT_VISIBILITY_STORAGE_KEY, "{not-json");

    render(<App />);

    await openAgents(user);
    for (const name of agentNavigationNames) {
      expect(screen.getByRole("button", { name })).toBeInTheDocument();
    }
    expect(screen.queryByRole("button", { name: /Future/i })).toBeNull();
  });

  it("does not overwrite an existing preference when the initial storage read fails", async () => {
    const user = userEvent.setup();
    window.localStorage.setItem(
      AGENT_VISIBILITY_STORAGE_KEY,
      JSON.stringify(["codex"]),
    );
    const originalGetItem = Storage.prototype.getItem;
    const setItemSpy = vi.spyOn(Storage.prototype, "setItem");
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(function (
      this: Storage,
      key: string,
    ) {
      if (key === AGENT_VISIBILITY_STORAGE_KEY) throw new Error("storage denied");
      return originalGetItem.call(this, key);
    });

    render(<App />);

    await openAgents(user);
    expect(screen.getByRole("button", { name: "Codex" })).toBeInTheDocument();
    expect(setItemSpy).not.toHaveBeenCalledWith(
      AGENT_VISIBILITY_STORAGE_KEY,
      expect.any(String),
    );
    expect(originalGetItem.call(
      window.localStorage,
      AGENT_VISIBILITY_STORAGE_KEY,
    )).toBe(JSON.stringify(["codex"]));
  });

  it("keeps the current-session visibility change when preference persistence fails", async () => {
    const user = userEvent.setup();
    render(<App />);
    await openAgentVisibility(user);
    vi.spyOn(Storage.prototype, "setItem").mockImplementationOnce(() => {
      throw new Error("storage denied");
    });

    await user.click(screen.getByRole("switch", { name: "Codex", checked: true }));

    expect(screen.getByRole("switch", { name: "Codex", checked: false })).toBeInTheDocument();
  });

  it("keeps a hidden Agent closed when returning to Overview", async () => {
    const user = userEvent.setup();
    render(<App />);

    await openAgent(user, "Codex");

    await openAgentVisibility(user);
    await user.click(screen.getByRole("switch", { name: "Codex", checked: true }));
    await user.click(navigation().getByRole("button", { name: "概览" }));

    expect(await screen.findByRole("heading", { name: "概览" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Codex" })).toBeNull();
  });

  it("supports toggling Agent visibility with the Space key", async () => {
    const user = userEvent.setup();
    render(<App />);
    await openAgentVisibility(user);

    const codexToggle = screen.getByRole("switch", { name: "Codex", checked: true });
    codexToggle.focus();
    expect(codexToggle).toHaveFocus();
    await user.keyboard("[Space]");

    expect(screen.getByRole("switch", { name: "Codex", checked: false })).toHaveFocus();
    await user.keyboard("[Enter]");

    expect(screen.getByRole("switch", { name: "Codex", checked: true })).toHaveFocus();
    expect(window.localStorage.getItem(AGENT_VISIBILITY_STORAGE_KEY)).toBe(
      JSON.stringify([]),
    );
  });

  it("keeps core navigation and the recovery controls available when every Agent is hidden", async () => {
    const user = userEvent.setup();
    render(<App />);
    await openAgentVisibility(user);

    for (const name of agentDisplayNames) {
      await user.click(screen.getByRole("switch", { name, checked: true }));
    }

    for (const name of agentNavigationNames) {
      expect(screen.queryByRole("button", { name })).toBeNull();
    }
    expect(navigation().getByRole("button", { name: "概览" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "用量" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "设置" })).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent("0 / 7 已显示");
    expect(screen.getByRole("switch", { name: "Claude Code", checked: false })).toBeInTheDocument();

    await user.click(screen.getByRole("switch", { name: "Claude Code", checked: false }));
    expect(screen.getByRole("status")).toHaveTextContent("1 / 7 已显示");
  });

  it("silently ignores a backend scan already in progress", async () => {
    const user = userEvent.setup();
    let scans = 0;
    invokeMock.mockImplementation(async (command) => {
      if (command === "get_state") return stateFixture();
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") {
        scans += 1;
        if (scans === 1) return [];
        throw { message: "Agent 扫描正在进行", code: "scan_in_progress" };
      }
      throw new Error(`unexpected IPC command: ${command}`);
    });

    render(<App />);
    await waitFor(() => expect(scans).toBe(1));
    await openAgents(user);
    await user.click(screen.getByRole("button", { name: "重新扫描" }));
    await waitFor(() => expect(scans).toBe(2));

    expect(screen.queryByText(/Agent 扫描正在进行/)).toBeNull();
    expect(screen.getByRole("button", { name: "重新扫描" })).toBeEnabled();
  });

  it("queues an overlapping rescan and only commits the newest generation", async () => {
    const user = userEvent.setup();
    let resolveSlow!: (agents: AgentView[]) => void;
    let resolveNewest!: (agents: AgentView[]) => void;
    const slow = new Promise<AgentView[]>((resolve) => { resolveSlow = resolve; });
    const newestScan = new Promise<AgentView[]>((resolve) => { resolveNewest = resolve; });
    let scans = 0;
    invokeMock.mockImplementation(async (command) => {
      if (command === "get_state") {
        return stateFixture({
          serve: serveFixture({
            phase: "running", app_runtime: "running", listener_reachable: true,
            running_revision: 1, instance_id: "instance-overlap", virtual_key: "vk-overlap",
          }),
        });
      }
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") {
        scans += 1;
        if (scans === 1) return [scannedClaude];
        return scans === 2 ? slow : newestScan;
      }
      if (command === "plan_agent_connection") {
        return projectionPlan("op-overlap", "token-overlap");
      }
      if (command === "apply_agent_plan") {
        return { operation_id: "op-overlap", maintenance_warning: null };
      }
      throw new Error(`unexpected IPC command: ${command}`);
    });

    render(<App />);
    await waitFor(() => expect(scans).toBe(1));
    await openAgents(user);
    await user.click(screen.getByRole("button", { name: "重新扫描" }));
    await waitFor(() => expect(scans).toBe(2));
    await user.click(screen.getByRole("button", { name: "Claude Code" }));
    await user.click(screen.getByRole("button", { name: "一键接入" }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith(
      "apply_agent_plan",
      { operationId: "op-overlap", confirmationToken: "token-overlap" },
    ));
    expect(scans).toBe(2);

    resolveSlow([scannedClaude]);
    await waitFor(() => expect(scans).toBe(3));
    const newest = structuredClone(scannedClaude);
    const versionTen = structuredClone(scannedClaude.installations[0]);
    versionTen.discovery.executable_path = "/opt/claude-10";
    versionTen.discovery.canonical_path = "/opt/claude-10";
    versionTen.discovery.version_raw = "10.0.0";
    versionTen.discovery.version_normalized = "10.0.0";
    versionTen.compatibility.installation_path = "/opt/claude-10";
    const versionEleven = structuredClone(versionTen);
    versionEleven.discovery.executable_path = "/opt/claude-11";
    versionEleven.discovery.canonical_path = "/opt/claude-11";
    versionEleven.discovery.version_raw = "11.0.0";
    versionEleven.discovery.version_normalized = "11.0.0";
    versionEleven.compatibility.installation_path = "/opt/claude-11";
    newest.installations = [versionTen, versionEleven];
    newest.status = "MULTIPLE_INSTALLATIONS";
    resolveNewest([newest]);

    await user.click(await screen.findByRole("button", { name: /选择安装/ }));
    expect(screen.getByRole("option", { name: "claude-10 · v10.0.0" })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "claude-11 · v11.0.0" })).toBeInTheDocument();
    expect(screen.queryByText("9.9.9")).toBeNull();
  });

  it("keeps usage independent and puts router, plugins and about inside Settings", async () => {
    const user = userEvent.setup();
    invokeMock.mockImplementation(async (command) => {
      if (command === "get_state") return stateFixture();
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return [];
      if (command === "get_stats") return { total: {}, groups: [], by: null, empty: true };
      if (command === "get_router_table") return { default_pool: "", assumed_context_window: 8192, threshold: null, rules: [], hint_routes: [], bands: [], pools: [] };
      throw new Error(`unexpected IPC command: ${command}`);
    });
    render(<App />);
    await screen.findByRole("heading", { name: "概览" });

    await user.click(screen.getByRole("button", { name: "用量" }));
    expect(await screen.findByRole("heading", { name: "用量统计" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "设置" }));
    expect(await screen.findByRole("heading", { name: "设置", level: 1 })).toBeInTheDocument();
    const settingsNavigation = screen.getByRole("navigation", { name: "设置分类" });
    expect(within(settingsNavigation).getByRole("button", { name: /通用/ })).toHaveAttribute("aria-current", "page");
    expect(within(settingsNavigation).getByRole("button", { name: /路由表/ })).toBeInTheDocument();
    expect(within(settingsNavigation).getByRole("button", { name: /插件/ })).toBeInTheDocument();
    expect(within(settingsNavigation).getByRole("button", { name: /关于/ })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /返回/ })).toBeNull();
    expect(screen.queryByRole("button", { name: /用量/ })).not.toBeNull();

    await user.click(navigation().getByRole("button", { name: "用量" }));
    expect(await screen.findByRole("heading", { name: "用量统计" })).toBeInTheDocument();
    await user.click(navigation().getByRole("button", { name: "概览" }));
    expect(await screen.findByRole("heading", { name: "概览" })).toBeInTheDocument();
  });

  it("renders the primary desktop surface in English by default", async () => {
    const user = userEvent.setup();
    window.localStorage.removeItem(LANGUAGE_STORAGE_KEY);
    render(<App />);

    expect(await screen.findByRole("heading", { name: "Overview" })).toBeInTheDocument();
    await openRouting(user);
    expect(screen.getByRole("heading", { name: "Smart routing" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Save and apply" })).toBeInTheDocument();
    await user.click(navigation().getByRole("button", { name: "Providers" }));
    expect(await screen.findByRole("heading", { name: "Providers", level: 1 })).toBeInTheDocument();
    expect(screen.queryByText("主页路由")).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Usage" }));
    expect(await screen.findByRole("heading", { name: "Usage" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Settings" }));
    expect(await screen.findByRole("heading", { name: "Settings", level: 1 })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /Agent visibility/ }));
    expect(await screen.findByRole("heading", { name: "Agent visibility" })).toBeInTheDocument();
    expect(screen.getByText(
      "Choose which Agents appear in the sidebar.",
    )).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent("7 / 7 visible");
    expect(screen.getByRole("group", { name: "Agent visibility options" })).toBeInTheDocument();
    expect(screen.getByRole("switch", { name: "Codex", checked: true })).toBeInTheDocument();
    await user.click(navigation().getByRole("button", { name: "Providers" }));
    await user.click(screen.getByRole("button", { name: "Add provider" }));
    expect(await screen.findByRole("heading", { name: "Add provider" })).toBeInTheDocument();
    expect(screen.getByText("MiniMax (China)")).toBeInTheDocument();
    expect(screen.queryByText("MiniMax（中国）")).not.toBeInTheDocument();
  });

  it("switches the whole interface to Simplified Chinese and persists the choice", async () => {
    const user = userEvent.setup();
    window.localStorage.removeItem(LANGUAGE_STORAGE_KEY);
    render(<App />);

    await user.click(await screen.findByRole("button", { name: "Settings" }));
    await user.click(screen.getByRole("button", { name: /Language/ }));
    expect(screen.getByRole("heading", { name: "Interface language" })).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: /English/ })).toHaveAttribute("aria-checked", "true");

    await user.click(screen.getByRole("radio", { name: /简体中文/ }));

    expect(screen.getByRole("heading", { name: "设置", level: 1 })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "设置" })).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: /简体中文/ })).toHaveAttribute("aria-checked", "true");
    await user.click(screen.getByRole("button", { name: /Agent 显示/ }));
    expect(await screen.findByRole("heading", { name: "Agent 显示" })).toBeInTheDocument();
    expect(screen.getByText(
      "选择显示在左侧导航中的 Agent。",
    )).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent("7 / 7 已显示");
    expect(screen.getByRole("group", { name: "Agent 显示选项" })).toBeInTheDocument();
    await user.click(navigation().getByRole("button", { name: "概览" }));
    expect(await screen.findByRole("heading", { name: "概览" })).toBeInTheDocument();
    expect(window.localStorage.getItem(LANGUAGE_STORAGE_KEY)).toBe("zh-CN");
    expect(document.documentElement).toHaveAttribute("lang", "zh-CN");
  });

  it("moves virtual key to Settings, masks it, and starts or stops the proxy from the top bar", async () => {
    const user = userEvent.setup();
    const writeText = vi.spyOn(navigator.clipboard, "writeText");
    const running = stateFixture({
      serve: serveFixture({ phase: "running", app_runtime: "running", listener_reachable: true, running_revision: 1, instance_id: "instance", virtual_key: "vk-test-secret" }),
    });
    invokeMock.mockImplementation(async (command) => {
      if (command === "get_state") return stateFixture();
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return [];
      if (command === "serve_start") return running;
      if (command === "serve_stop") return stateFixture();
      throw new Error(`unexpected IPC command: ${command}`);
    });
    render(<App />);
    await user.click(await screen.findByRole("button", { name: /启动/ }));
    expect(await screen.findByText("代理运行中")).toBeInTheDocument();
    expect(screen.queryByText("vk-test-secret")).toBeNull();
    await user.click(screen.getByRole("button", { name: "设置" }));
    expect(screen.getByLabelText("虚拟 API Key")).toHaveTextContent("••••");
    await user.click(screen.getByRole("button", { name: "复制" }));
    expect(writeText).toHaveBeenCalledWith("vk-test-secret");
    expect(await screen.findByRole("button", { name: "已复制" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "显示 15 秒" }));
    expect(screen.getByText("vk-test-secret")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /停止/ }));
    expect(await screen.findByRole("button", { name: /启动/ })).toBeInTheDocument();
  });

  it("applies the home route to all Agents with a dedicated command", async () => {
    const user = userEvent.setup();
    invokeMock.mockImplementation(async (command) => {
      if (["get_state", "apply_home_route_to_all_agents"].includes(command)) return stateFixture();
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return [];
      throw new Error(`unexpected IPC command: ${command}`);
    });
    render(<App />);
    await openRouting(user);
    await user.click(await screen.findByRole("button", { name: "应用到全部 Agent" }));
    expect(invokeMock).toHaveBeenCalledWith("apply_home_route_to_all_agents");
    expect(await screen.findByText("全部 Agent 已恢复跟随主页")).toBeInTheDocument();
  });

  it("lets one Agent switch to an independent route using the same tier selects", async () => {
    const user = userEvent.setup();
    const provider = { name: "deepseek", provider: "openai-compatible", base_url: "https://example.test/v1", models: ["deepseek-v4-pro"], has_auth: true };
    let current = stateFixture({ providers: [provider] });
    invokeMock.mockImplementation(async (command, args) => {
      if (command === "get_state") return current;
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return [];
      if (command === "set_agent_route_mode") {
        const { agentId, mode } = args as { agentId: string; mode: "inherit" | "custom" };
        current = { ...current, agent_routes: { ...current.agent_routes, [agentId]: { ...current.agent_routes[agentId], mode } } };
        return current;
      }
      if (command === "set_agent_tier") return current;
      throw new Error(`unexpected IPC command: ${command}`);
    });
    render(<App />);
    await openAgent(user, "Codex");
    await user.click(screen.getByRole("radio", { name: "独立路由" }));
    await user.click(screen.getByLabelText("上档供应商"));
    await user.click(screen.getByRole("option", { name: "deepseek" }));
    expect(invokeMock).toHaveBeenCalledWith("set_agent_tier", {
      agentId: "codex",
      slot: "high",
      upstream: "deepseek",
      model: "deepseek-v4-pro",
    });
  });

  it("returns home without promoting or discarding an incomplete Agent editor", async () => {
    const user = userEvent.setup();
    let current = stateFixture();
    invokeMock.mockImplementation(async (command, args) => {
      if (command === "get_state") return current;
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return [];
      if (command === "set_agent_route_mode") {
        const { agentId, mode } = args as { agentId: string; mode: "inherit" | "custom" };
        current = {
          ...current,
          agent_routes: {
            ...current.agent_routes,
            [agentId]: {
              ...current.agent_routes[agentId],
              mode,
              config_error: mode === "custom"
                ? "Agent `codex` 的 high 档缺少供应商和模型"
                : null,
            },
          },
        };
        return current;
      }
      throw new Error(`unexpected IPC command: ${command}`);
    });
    render(<App />);

    await openAgent(user, "Codex");
    await user.click(screen.getByRole("radio", { name: "独立路由" }));
    expect(screen.getByText("Agent `codex` 的 high 档缺少供应商和模型")).toBeInTheDocument();

    await user.click(navigation().getByRole("button", { name: "路由" }));

    expect(await screen.findByRole("heading", { name: "全局路由" })).toBeInTheDocument();
    expect(screen.queryByText(/配置结构不合法/)).toBeNull();
    expect(invokeMock).not.toHaveBeenCalledWith("save_agent_routes");

    await openAgent(user, "Codex");
    expect(screen.getByRole("radio", { name: "独立路由" })).toHaveAttribute(
      "aria-checked",
      "true",
    );
    expect(screen.getByRole("button", { name: "保存并重启" })).toBeDisabled();
  });

  it("saves the home route as a reusable profile and mounts it from an Agent page", async () => {
    const user = userEvent.setup();
    const provider = { name: "deepseek", provider: "openai-compatible", base_url: "https://example.test/v1", models: ["deepseek-chat"], has_auth: true };
    const configuredTiers = {
      high: { upstream: "deepseek", model: "deepseek-chat" },
      mid: { upstream: "deepseek", model: "deepseek-chat" },
      low: { upstream: "deepseek", model: "deepseek-chat" },
    };
    let current = stateFixture({ providers: [provider], tiers: configuredTiers });
    invokeMock.mockImplementation(async (command, args) => {
      if (command === "get_state") return current;
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return [];
      if (command === "save_home_route_as_profile") {
        current = { ...current, profiles: [(args as { name: string }).name], config_dirty: true, draft_revision: 1 };
        return current;
      }
      if (command === "mount_agent_profile") {
        const { agentId, profile } = args as { agentId: string; profile: string };
        current = {
          ...current,
          agent_routes: {
            ...current.agent_routes,
            [agentId]: { ...current.agent_routes[agentId], mode: "profile", profile, tiers: configuredTiers },
          },
          draft_revision: 2,
        };
        return current;
      }
      throw new Error(`unexpected IPC command: ${command}`);
    });

    render(<App />);
    await openRouting(user);
    await user.click(await screen.findByRole("button", { name: "存为策略" }));
    await user.type(await screen.findByLabelText("策略组名称"), "日常开发");
    await user.click(screen.getByRole("button", { name: "保存策略" }));
    expect(invokeMock).toHaveBeenCalledWith("save_home_route_as_profile", { name: "日常开发" });
    expect(await screen.findByText("策略组“日常开发”已加入草稿，请保存并应用。")).toBeInTheDocument();

    await openAgent(user, "Codex");
    await user.click(screen.getByRole("radio", { name: "挂载策略组" }));
    expect(invokeMock).toHaveBeenCalledWith("mount_agent_profile", { agentId: "codex", profile: "日常开发" });
    expect(await screen.findByText("已挂载策略组「日常开发」· 尚待保存并应用")).toBeInTheDocument();
    expect(screen.getByLabelText("当前策略组")).toHaveValue("日常开发");
  });

  it("serializes profile mutations with the rest of the home route commands", async () => {
    const user = userEvent.setup();
    let finishSave: ((value: ReturnType<typeof stateFixture>) => void) | undefined;
    const current = stateFixture();
    invokeMock.mockImplementation(async (command) => {
      if (command === "get_state") return current;
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return [];
      if (command === "save_home_route_as_profile") {
        return new Promise((resolve) => {
          finishSave = resolve;
        });
      }
      if (command === "serve_start") return current;
      throw new Error(`unexpected IPC command: ${command}`);
    });

    render(<App />);
    await openRouting(user);
    await user.click(await screen.findByRole("button", { name: "存为策略" }));
    await user.type(screen.getByLabelText("策略组名称"), "并发保护");
    await user.click(screen.getByRole("button", { name: "保存策略" }));

    expect(screen.getByRole("button", { name: "保存并应用" })).toBeDisabled();
    await user.click(screen.getByRole("button", { name: "保存并应用" }));
    expect(invokeMock).not.toHaveBeenCalledWith("serve_start");

    finishSave?.({ ...current, profiles: ["并发保护"] });
    expect(await screen.findByText("策略组“并发保护”已加入草稿，请保存并应用。")).toBeInTheDocument();
  });

  it("opens Add Provider as a separate page and returns to the source page after saving", async () => {
    const user = userEvent.setup();
    invokeMock.mockImplementation(async (command) => {
      if (["get_state", "add_provider_with_credential"].includes(command)) return stateFixture();
      if (command === "preview_provider_endpoints") {
        return {
          chat: "https://api.openai.com/v1/chat/completions",
          responses: "https://api.openai.com/v1/responses",
          messages: "https://api.openai.com/v1/messages",
        };
      }
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return [];
      throw new Error(`unexpected IPC command: ${command}`);
    });
    render(<App />);
    await openAgent(user, "OpenCode");
    await user.click(screen.getByRole("button", { name: "添加供应商" }));
    expect(await screen.findByRole("heading", { name: "添加供应商" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "添加供应商" })).toBeNull();
    // The provider picker is a brand-card catalog; click by visible label instead of selecting an option.
    await user.click(screen.getByText("OpenAI", { selector: ".provider-catalog-card-title strong" }));
    expect(screen.getByRole("button", { name: "添加供应商" })).toBeInTheDocument();
    await user.type(screen.getByLabelText("API Key"), "secret-test");
    await user.click(screen.getByRole("button", { name: "添加供应商" }));
    expect(await screen.findByRole("heading", { name: "OpenCode" })).toBeInTheDocument();
    expect(screen.getByText("供应商已添加")).toBeInTheDocument();
  });

  it("uses one provider catalog for regular and free APIs and restores the selected mode", async () => {
    const user = userEvent.setup();
    invokeMock.mockImplementation(async (command) => {
      if (command === "get_state") return stateFixture();
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return [];
      if (command === "list_free_provider_presets") {
        return [{
          id: "nvidia",
          upstream_name: "nvidia_free",
          label: "NVIDIA API Catalog",
          short_label: "NV",
          base_url: "https://integrate.api.nvidia.com/v1",
          offer_kind: "recurring",
          region: "global",
          tags: ["长期免费", "全球平台", "开发用途"],
          free_note: "build.nvidia.com 托管 API",
          key_instruction: "打开模型页面并点击 Get API Key。",
          application_url: "https://build.nvidia.com/",
          docs_url: "https://docs.example.com/nvidia",
          verified_at: "2026-07-27",
          overage_policy: "rate_limited",
          models: [{
            id: "openai/gpt-oss-120b",
            label: "GPT-OSS 120B",
            tool: "declared",
            vision: "unknown",
            json_schema: "declared",
            context_window: 131072,
          }],
        }];
      }
      throw new Error(`unexpected IPC command: ${command}`);
    });

    render(<App />);
    await user.click(await screen.findByRole("button", { name: "添加供应商" }));
    expect(await screen.findByRole("heading", { name: "添加供应商" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /常规 API/ })).toHaveAttribute("aria-pressed", "true");

    await user.click(screen.getByRole("button", { name: /免费 API/ }));
    expect(screen.getByRole("button", { name: /免费 API/ })).toHaveAttribute("aria-pressed", "true");
    expect(await screen.findByText("NVIDIA API Catalog")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: /NVIDIA API Catalog/ }));
    expect(await screen.findByRole("heading", { name: "NVIDIA API Catalog" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "返回" }));
    expect(await screen.findByRole("heading", { name: "添加供应商" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /免费 API/ })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByText("NVIDIA API Catalog")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: /常规 API/ }));
    expect(screen.getByRole("button", { name: /常规 API/ })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByText("OpenAI", { selector: ".provider-catalog-card-title strong" })).toBeInTheDocument();
  });

  it("applies the Connector plan directly on 一键接入", async () => {
    const user = userEvent.setup();
    const running = stateFixture({ serve: serveFixture({ phase: "running", app_runtime: "running", listener_reachable: true, running_revision: 1, instance_id: "instance", virtual_key: "vk-test" }) });
    let scans = 0;
    invokeMock.mockImplementation(async (command) => {
      if (command === "get_state") return running;
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") { scans += 1; return [scannedClaude]; }
      if (command === "plan_agent_connection") return {
        operation_id: "op-1",
        confirmation_token: "token-1",
        intent: "connect",
        target_config_path: "/tmp/settings.json",
        related_config_paths: [],
        human_diff: "~ /env/ANTHROPIC_BASE_URL: <设置受管值>\n~ /env/ANTHROPIC_AUTH_TOKEN: <敏感值已隐藏>",
        changes: [
          { operation: "replace", path: { segments: ["env", "ANTHROPIC_BASE_URL"] }, sensitive: false, summary: "<设置受管值>" },
          { operation: "replace", path: { segments: ["env", "ANTHROPIC_AUTH_TOKEN"] }, sensitive: true, summary: "<敏感值已隐藏>" },
        ],
      };
      if (command === "apply_agent_plan") return { operation_id: "op-1", maintenance_warning: null };
      throw new Error(`unexpected IPC command: ${command}`);
    });
    render(<App />);
    await waitFor(() => expect(scans).toBe(1));
    await openAgent(user, "Claude Code");
    expect(screen.queryByRole("button", { name: /选择安装/ })).toBeNull();
    await user.click(await screen.findByRole("button", { name: "一键接入" }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("plan_agent_connection", {
      agentId: "claude-code",
      installationPath: "/opt/claude",
      expectedVersion: "9.9.9",
    }));
    // There is no separate write-confirmation step; apply immediately after planning.
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("apply_agent_plan", { operationId: "op-1", confirmationToken: "token-1" }));
    expect(await screen.findByText("Agent 已接入")).toBeInTheDocument();
    expect(scans).toBe(2);
  });

  it("applies directly for an admitted state", async () => {
    const user = userEvent.setup();
    const running = stateFixture({ serve: serveFixture({ phase: "running", app_runtime: "running", listener_reachable: true, running_revision: 1, instance_id: "instance", virtual_key: "vk-test" }) });
    const admitted = defaultAdmittedClaude();
    invokeMock.mockImplementation(async (command) => {
      if (command === "get_state") return running;
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return [admitted];
      if (command === "plan_agent_connection") return projectionPlan("op-admitted", "token-admitted");
      if (command === "apply_agent_plan") return { operation_id: "op-admitted", maintenance_warning: null };
      throw new Error(`unexpected IPC command: ${command}`);
    });

    render(<App />);
    await openAgent(user, "Claude Code");
    expect(await screen.findByText("可接入")).toBeInTheDocument();
    expect(screen.queryByText(/未经验证|试验性/)).toBeNull();
    await user.click(screen.getByRole("button", { name: "一键接入" }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("plan_agent_connection", {
      agentId: "claude-code",
      installationPath: "/opt/claude",
      expectedVersion: "2.1.210",
    }));
    // Apply immediately after planning without a confirmation step.
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("apply_agent_plan", {
      operationId: "op-admitted",
      confirmationToken: "token-admitted",
    }));
  });

  it("restores the encrypted baseline directly on 恢复原始配置", async () => {
    const user = userEvent.setup();
    const connected = structuredClone(scannedClaude);
    connected.installations[0].managed = true;
    connected.installations[0].connected = true;
    connected.installations[0].compatibility.status = "CONNECTED";
    connected.status = "CONNECTED";
    invokeMock.mockImplementation(async (command) => {
      if (command === "get_state") return stateFixture();
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return [connected];
      if (command === "get_agent_drift") return [];
      if (command === "plan_agent_disconnect") {
        return {
          ...projectionPlan("op-restore", "token-restore", "disconnect"),
          human_diff: "~ /env/ANTHROPIC_AUTH_TOKEN: <恢复受管敏感值，内容已隐藏>",
          changes: [{
            operation: "replace",
            path: { segments: ["env", "ANTHROPIC_AUTH_TOKEN"] },
            sensitive: true,
            summary: "<恢复受管敏感值，内容已隐藏>",
          }],
        };
      }
      if (command === "apply_agent_plan") return { operation_id: "op-restore", maintenance_warning: null };
      throw new Error(`unexpected IPC command: ${command}`);
    });

    render(<App />);
    await openAgent(user, "Claude Code");
    await user.click(await screen.findByRole("button", { name: "恢复 Agent 原始配置" }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("plan_agent_disconnect", {
      agentId: "claude-code",
      installationPath: "/opt/claude",
    }));
    // Apply immediately after planning without a confirmation step.
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("apply_agent_plan", {
      operationId: "op-restore",
      confirmationToken: "token-restore",
    }));
    expect(await screen.findByText("已恢复接入前的 Agent 配置")).toBeInTheDocument();

    await openAgent(user, "OpenCode");
    expect(screen.queryByText("已恢复接入前的 Agent 配置")).not.toBeInTheDocument();
  });

  it("selects an exact installation and plans against its path", async () => {
    const user = userEvent.setup();
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
    const secondInstallation = structuredClone(scannedClaude.installations[0]);
    secondInstallation.discovery.executable_path = "/Users/x/.local/lib/node_modules/@anthropic-ai/claude-code/bin/claude.exe";
    secondInstallation.discovery.canonical_path = "/Users/x/.local/lib/node_modules/@anthropic-ai/claude-code/bin/claude.exe";
    secondInstallation.discovery.version_raw = "10.0.0";
    secondInstallation.discovery.version_normalized = "10.0.0";
    secondInstallation.discovery.binary_source = "npm_global";
    secondInstallation.discovery.upgrade_command = "npm install --global @anthropic-ai/claude-code@latest";
    secondInstallation.compatibility.installation_path = secondInstallation.discovery.canonical_path;
    const multipleClaude: AgentView = {
      ...scannedClaude,
      status: "MULTIPLE_INSTALLATIONS",
      installations: [scannedClaude.installations[0], secondInstallation],
    };
    const running = stateFixture({ serve: serveFixture({ phase: "running", app_runtime: "running", listener_reachable: true, running_revision: 1, instance_id: "instance", virtual_key: "vk-test" }) });
    invokeMock.mockImplementation(async (command) => {
      if (command === "get_state") return running;
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return [multipleClaude];
      if (command === "plan_agent_connection") return projectionPlan("op-2", "token-2");
      if (command === "apply_agent_plan") return { operation_id: "op-2", maintenance_warning: null };
      throw new Error(`unexpected IPC command: ${command}`);
    });

    render(<App />);
    await openAgent(user, "Claude Code");
    expect(await screen.findByRole("button", { name: "一键接入" })).toBeDisabled();
    expect(screen.getByText("检测到多份安装，请先选择要接管的精确路径。")).toBeInTheDocument();
    await user.click(await screen.findByRole("button", { name: /选择安装/ }));
    await user.click(screen.getByRole("option", { name: "claude.exe · v10.0.0" }));
    expect(screen.queryByRole("listbox")).toBeNull();
    await user.click(screen.getByRole("button", { name: "一键接入" }));
    expect(invokeMock).toHaveBeenCalledWith("plan_agent_connection", {
      agentId: "claude-code",
      installationPath: secondInstallation.discovery.canonical_path,
      expectedVersion: "10.0.0",
    });
  });
});
