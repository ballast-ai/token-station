import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import App, { configSaveStatus } from "./App";
import type { AgentRouteView, AgentUiMetadataView, AgentView, ServeView, StateView } from "./api";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
const invokeMock = vi.mocked(invoke);
const listenMock = vi.mocked(listen);

const agentIds = ["claude-code", "codex", "opencode", "openclaw", "nous-hermes-agent"];
const emptyRoute: AgentRouteView = {
  mode: "inherit",
  tiers: {
    high: { upstream: null, model: null },
    mid: { upstream: null, model: null },
    low: { upstream: null, model: null },
  },
  config_error: null,
  profile: null,
};

const registryFixture: AgentUiMetadataView[] = [
  { agent_id: "claude-code", legacy_kind: "cc", display_name: "Claude Code", icon_key: "claude", admission: "supported", ui_order: 10, nav_mark: "C" },
  { agent_id: "codex", legacy_kind: "codex", display_name: "Codex", icon_key: "codex", admission: "supported", ui_order: 20, nav_mark: "X" },
  { agent_id: "opencode", legacy_kind: "opencode", display_name: "OpenCode", icon_key: "opencode", admission: "supported", ui_order: 30, nav_mark: "O" },
  { agent_id: "openclaw", legacy_kind: null, display_name: "OpenClaw", icon_key: "openclaw", admission: "supported", ui_order: 40, nav_mark: "OC" },
  { agent_id: "nous-hermes-agent", legacy_kind: null, display_name: "Hermes Agent", icon_key: "hermes", admission: "supported", ui_order: 50, nav_mark: "H" },
  { agent_id: "future-agent", legacy_kind: null, display_name: "Future Agent", icon_key: "future", admission: "discovery_only" },
];

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
    agent_routes: Object.fromEntries(agentIds.map((id) => [id, structuredClone(emptyRoute)])),
    profiles: [],
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
  return within(screen.getByLabelText("主导航"));
}

beforeEach(() => {
  listenMock.mockReset();
  listenMock.mockResolvedValue(vi.fn());
  const initial = stateFixture();
  invokeMock.mockImplementation(async (command) => {
    if (command === "get_state") return initial;
    if (command === "list_agent_registry") return registryFixture;
    if (command === "scan_agents") return [];
    throw new Error(`unexpected IPC command: ${command}`);
  });
});

it("renders a supported virtual Agent entirely from registry metadata", async () => {
  invokeMock.mockImplementation(async (command) => {
    if (command === "get_state") return stateFixture();
    if (command === "list_agent_registry") return registryWithVirtualSupportedAgent;
    if (command === "scan_agents") return [];
    throw new Error(`unexpected IPC command: ${command}`);
  });

  render(<App />);

  const nav = within(await screen.findByLabelText("主导航"));
  expect(nav.getByRole("button", { name: "Virtual" })).toHaveAttribute("title", "Virtual Agent · idle");
  expect(nav.getByText("V")).toBeInTheDocument();
});

describe("desktop station navigation", () => {
  it("maps the four revision relationships to stable save copy", () => {
    expect(configSaveStatus(stateFixture())).toBe("无改动");
    expect(configSaveStatus(stateFixture({ config_dirty: true, draft_revision: 2, saved_revision: 1 }))).toBe("有未保存更改");
    expect(configSaveStatus(stateFixture({
      saved_revision: 2,
      serve: serveFixture({ app_runtime: "running", listener_reachable: true, running_revision: 1 }),
    }))).toBe("已保存尚未应用");
    expect(configSaveStatus(stateFixture({
      saved_revision: 2,
      serve: serveFixture({ app_runtime: "running", listener_reachable: true, running_revision: 2 }),
    }))).toBe("运行中 revision 2");
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
    expect(await screen.findByTestId("config-save-status")).toHaveTextContent("有未保存更改");
    await user.click(screen.getByRole("button", { name: "保存并应用" }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("serve_start"));
    expect(invokeMock).not.toHaveBeenCalledWith("save_config");
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

  it("shows exactly five fixed Agents, no Gemini, and scans only on load or explicit rescan", async () => {
    const user = userEvent.setup();
    render(<App />);
    await screen.findByRole("heading", { name: "主页路由" });
    for (const name of ["Claude Code", "Codex", "OpenCode", "OpenClaw", "Hermes"]) {
      expect(navigation().getByRole("button", { name })).toBeInTheDocument();
    }
    expect(navigation().queryByRole("button", { name: /Gemini/i })).toBeNull();
    expect(navigation().queryByRole("button", { name: /Future/i })).toBeNull();
    await waitFor(() => expect(invokeMock.mock.calls.filter(([command]) => command === "scan_agents")).toHaveLength(1));

    await user.click(navigation().getByRole("button", { name: "Codex" }));
    expect(await screen.findByRole("heading", { name: "Codex" })).toBeInTheDocument();
    await user.click(navigation().getByRole("button", { name: "主页" }));
    expect(invokeMock.mock.calls.filter(([command]) => command === "scan_agents")).toHaveLength(1);
    await user.click(navigation().getByRole("button", { name: "重新扫描" }));
    await waitFor(() => expect(invokeMock.mock.calls.filter(([command]) => command === "scan_agents")).toHaveLength(2));
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
    await user.click(navigation().getByRole("button", { name: "重新扫描" }));
    await waitFor(() => expect(scans).toBe(2));

    expect(screen.queryByText(/Agent 扫描正在进行/)).toBeNull();
    expect(navigation().getByRole("button", { name: "重新扫描" })).toBeEnabled();
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
    await user.click(navigation().getByRole("button", { name: "重新扫描" }));
    await waitFor(() => expect(scans).toBe(2));
    await user.click(navigation().getByRole("button", { name: "Claude Code" }));
    await user.click(screen.getByRole("button", { name: "一键接入" }));
    await user.click(within(await screen.findByRole("dialog", { name: "配置投影预览" }))
      .getByRole("button", { name: "确认并应用" }));
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
    await screen.findByRole("heading", { name: "主页路由" });

    await user.click(screen.getByRole("button", { name: "用量" }));
    expect(await screen.findByRole("heading", { name: "用量统计" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "设置" }));
    expect(await screen.findByRole("heading", { name: "设置", level: 1 })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /路由表/ })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /插件/ })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /关于/ })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /用量/ })).not.toBeNull();
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
    await user.click(await screen.findByRole("button", { name: "启动" }));
    expect(await screen.findByText("代理运行中")).toBeInTheDocument();
    expect(screen.queryByText("vk-test-secret")).toBeNull();
    await user.click(screen.getByRole("button", { name: "设置" }));
    expect(screen.getByLabelText("虚拟 API Key")).toHaveTextContent("••••");
    await user.click(screen.getByRole("button", { name: "复制" }));
    expect(writeText).toHaveBeenCalledWith("vk-test-secret");
    expect(await screen.findByRole("button", { name: "已复制" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "显示 15 秒" }));
    expect(screen.getByText("vk-test-secret")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "停止" }));
    expect(await screen.findByRole("button", { name: "启动" })).toBeInTheDocument();
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
    await screen.findByRole("heading", { name: "主页路由" });
    await user.click(navigation().getByRole("button", { name: "Codex" }));
    await user.click(screen.getByRole("radio", { name: "独立路由" }));
    await user.selectOptions(screen.getByLabelText("上档供应商"), "deepseek");
    expect(invokeMock).toHaveBeenCalledWith("set_agent_tier", {
      agentId: "codex",
      slot: "high",
      upstream: "deepseek",
      model: "deepseek-v4-pro",
    });
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
    await user.type(await screen.findByLabelText("策略组名称"), "日常开发");
    await user.click(screen.getByRole("button", { name: "另存为策略组" }));
    expect(invokeMock).toHaveBeenCalledWith("save_home_route_as_profile", { name: "日常开发" });
    expect(await screen.findByText("策略组「日常开发」已加入草稿，请保存并应用")).toBeInTheDocument();

    await user.click(navigation().getByRole("button", { name: "Codex" }));
    await user.click(screen.getByRole("radio", { name: "挂载策略组" }));
    expect(invokeMock).toHaveBeenCalledWith("mount_agent_profile", { agentId: "codex", profile: "日常开发" });
    expect(await screen.findByText("已挂载策略组「日常开发」· 尚待保存并应用")).toBeInTheDocument();
    expect(screen.getByLabelText("当前策略组")).toHaveValue("日常开发");
  });

  it("opens Add Provider as a separate page and returns to the source page after saving", async () => {
    const user = userEvent.setup();
    invokeMock.mockImplementation(async (command) => {
      if (["get_state", "add_provider"].includes(command)) return stateFixture();
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
    await screen.findByRole("heading", { name: "主页路由" });
    await user.click(navigation().getByRole("button", { name: "OpenCode" }));
    await user.click(screen.getByRole("button", { name: "添加供应商" }));
    expect(await screen.findByRole("heading", { name: "添加供应商" })).toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: "添加供应商" })).toHaveLength(1);
    await user.selectOptions(screen.getByLabelText("选择供应商"), "openai");
    await user.type(screen.getByPlaceholderText("只保存在系统钥匙串"), "secret-test");
    await user.click(screen.getByRole("button", { name: "添加供应商" }));
    expect(await screen.findByRole("heading", { name: "OpenCode" })).toBeInTheDocument();
    expect(screen.getByText("供应商已添加")).toBeInTheDocument();
  });

  it("previews the redacted Connector projection before applying it", async () => {
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
    await user.click(navigation().getByRole("button", { name: "Claude Code" }));
    expect(screen.getByText("/opt/claude")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /选择安装/ })).toBeNull();
    await user.click(await screen.findByRole("button", { name: "一键接入" }));
    expect(invokeMock).toHaveBeenCalledWith("plan_agent_connection", {
      agentId: "claude-code",
      installationPath: "/opt/claude",
      expectedVersion: "9.9.9",
    });
    const preview = await screen.findByRole("dialog", { name: "配置投影预览" });
    expect(preview).toHaveTextContent("/env/ANTHROPIC_BASE_URL");
    expect(preview).toHaveTextContent("敏感值已隐藏");
    expect(preview).not.toHaveTextContent("local-virtual-key");
    expect(invokeMock).not.toHaveBeenCalledWith("apply_agent_plan", expect.anything());
    await user.click(within(preview).getByRole("button", { name: "确认并应用" }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("apply_agent_plan", { operationId: "op-1", confirmationToken: "token-1" }));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(await screen.findByText("Agent 已接入")).toBeInTheDocument();
    expect(scans).toBe(2);
  });

  it("shows one admitted state and only applies after projection confirmation", async () => {
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
    await screen.findByLabelText("主导航");
    await user.click(navigation().getByRole("button", { name: "Claude Code" }));
    expect(await screen.findByText("可接入")).toBeInTheDocument();
    expect(screen.queryByText(/未经验证|试验性/)).toBeNull();
    await user.click(screen.getByRole("button", { name: "一键接入" }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("plan_agent_connection", {
      agentId: "claude-code",
      installationPath: "/opt/claude",
      expectedVersion: "2.1.210",
    }));
    expect(invokeMock).not.toHaveBeenCalledWith("apply_agent_plan", expect.anything());
    await user.click(within(await screen.findByRole("dialog", { name: "配置投影预览" }))
      .getByRole("button", { name: "确认并应用" }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("apply_agent_plan", {
      operationId: "op-admitted",
      confirmationToken: "token-admitted",
    }));
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("previews and confirms one-click restoration to the encrypted baseline", async () => {
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
        };
      }
      if (command === "apply_agent_plan") return { operation_id: "op-restore", maintenance_warning: null };
      throw new Error(`unexpected IPC command: ${command}`);
    });

    render(<App />);
    await user.click(within(await screen.findByLabelText("主导航"))
      .getByRole("button", { name: "Claude Code" }));
    await user.click(await screen.findByRole("button", { name: "恢复 Agent 原始配置" }));
    expect(invokeMock).toHaveBeenCalledWith("plan_agent_disconnect", {
      agentId: "claude-code",
      installationPath: "/opt/claude",
    });
    const preview = await screen.findByRole("dialog", { name: "配置投影预览" });
    expect(preview).toHaveTextContent("恢复受管敏感值，内容已隐藏");
    await user.click(within(preview).getByRole("button", { name: "确认并应用" }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("apply_agent_plan", {
      operationId: "op-restore",
      confirmationToken: "token-restore",
    }));
    expect(await screen.findByText("已恢复接入前的 Agent 配置")).toBeInTheDocument();
  });

  it("shows a read-only three-way drift ledger without configuration values", async () => {
    const user = userEvent.setup();
    invokeMock.mockImplementation(async (command, args) => {
      if (command === "get_state") return stateFixture();
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return [scannedClaude];
      if (command === "get_agent_drift") {
        expect(args).toEqual({ agentId: "claude-code", installationPath: "/opt/claude" });
        return [{
          agent_id: "claude-code",
          installation_path: "/opt/claude",
          target_config_path: "/tmp/settings.json",
          connector_id: "claude-code-v1",
          status: "managed_changes",
          baseline_hash: "a".repeat(64),
          managed_hash: "b".repeat(64),
          current_hash: "c".repeat(64),
          checked_at_ms: 1_784_700_000_000,
          changes: [{
            path: { segments: ["env", "ANTHROPIC_BASE_URL"] },
            scope: "managed",
            kind: "changed",
            current_matches_managed: false,
          }],
          truncated: false,
          message: "外部修改触及 Token Station 受管字段",
        }];
      }
      throw new Error(`unexpected IPC command: ${command}`);
    });

    render(<App />);
    const nav = within(await screen.findByLabelText("主导航"));
    await user.click(nav.getByRole("button", { name: "Claude Code" }));

    const panel = await screen.findByLabelText("配置漂移对账");
    expect(within(panel).getByText("接管前")).toBeInTheDocument();
    expect(within(panel).getByText("最后写入")).toBeInTheDocument();
    expect(within(panel).getByText("当前磁盘")).toBeInTheDocument();
    expect(within(panel).getByText("aaaaaaaaaaaa")).toBeInTheDocument();
    expect(within(panel).getByText("bbbbbbbbbbbb")).toBeInTheDocument();
    expect(within(panel).getByText("cccccccccccc")).toBeInTheDocument();
    expect(within(panel).getByText("/env/ANTHROPIC_BASE_URL")).toBeInTheDocument();
    expect(panel).not.toHaveTextContent("managed-secret");
    expect(panel).not.toHaveTextContent("external-secret");
    expect(screen.queryByRole("button", { name: /覆盖|保留外部改动/ })).toBeNull();
  });

  it("selects an exact installation and only copies its source-specific upgrade command", async () => {
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
    const nav = within(await screen.findByLabelText("主导航"));
    await user.click(nav.getByRole("button", { name: "Claude Code" }));
    expect(await screen.findByRole("button", { name: "一键接入" })).toBeDisabled();
    expect(screen.getByText("检测到多份安装，请先选择要接管的精确路径。")).toBeInTheDocument();
    await user.click(await screen.findByRole("button", { name: /选择安装/ }));
    await user.click(screen.getByRole("option", { name: "claude.exe · v10.0.0" }));
    expect(screen.queryByRole("listbox")).toBeNull();
    expect(document.body).toHaveTextContent(secondInstallation.discovery.canonical_path);
    expect(screen.getByText("npm install --global @anthropic-ai/claude-code@latest")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "复制升级命令" }));
    expect(writeText).toHaveBeenCalledWith("npm install --global @anthropic-ai/claude-code@latest");
    await user.click(screen.getByRole("button", { name: "一键接入" }));
    expect(invokeMock).toHaveBeenCalledWith("plan_agent_connection", {
      agentId: "claude-code",
      installationPath: secondInstallation.discovery.canonical_path,
      expectedVersion: "10.0.0",
    });
  });
});
