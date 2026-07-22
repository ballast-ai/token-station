import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import App from "./App";
import type { AgentRouteView, AgentUiMetadataView, AgentView, StateView } from "./api";

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
  { agent_id: "claude-code", legacy_kind: "cc", display_name: "Claude Code", icon_key: "claude", admission: "supported" },
  { agent_id: "codex", legacy_kind: "codex", display_name: "Codex", icon_key: "codex", admission: "supported" },
  { agent_id: "opencode", legacy_kind: "opencode", display_name: "OpenCode", icon_key: "opencode", admission: "supported" },
  { agent_id: "openclaw", legacy_kind: null, display_name: "OpenClaw", icon_key: "openclaw", admission: "supported" },
  { agent_id: "nous-hermes-agent", legacy_kind: null, display_name: "Hermes Agent", icon_key: "hermes", admission: "supported" },
  { agent_id: "future-agent", legacy_kind: null, display_name: "Future Agent", icon_key: "future", admission: "discovery_only" },
];

function stateFixture(overrides: Partial<StateView> = {}): StateView {
  return {
    providers: [],
    tiers: {
      high: { upstream: null, model: null },
      mid: { upstream: null, model: null },
      low: { upstream: null, model: null },
    },
    agent_routes: Object.fromEntries(agentIds.map((id) => [id, structuredClone(emptyRoute)])),
    serve: { phase: "stopped", running: false, listen: "127.0.0.1:8787", virtual_key: null, error: null },
    config_error: null,
    settings: {
      listen: "127.0.0.1:8787",
      auth: true,
      metrics: true,
      data_dir: "/tmp/token-station-test/data",
      plugins_dir: "/tmp/token-station-test/plugins",
      agent: "test-agent",
      version: "test-version",
    },
    dirty: false,
    applied: true,
    profiles: [],
    ...overrides,
  };
}

const scannedClaude: AgentView = {
  metadata: registryFixture[0],
  installations: [{
    connected: false,
    discovery: {
      agent_id: "claude-code",
      executable_path: "/opt/claude",
      canonical_path: "/opt/claude",
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
      reason_code: "VerifiedRangeMatch",
      message: "版本命中已验证兼容范围",
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

function experimentalClaude(): AgentView {
  const value = structuredClone(scannedClaude);
  value.status = "DETECTED_UNKNOWN";
  value.installations[0].discovery.version_raw = "2.1.210";
  value.installations[0].discovery.version_normalized = "2.1.210";
  value.installations[0].compatibility.status = "DETECTED_UNKNOWN";
  value.installations[0].compatibility.reason_code = "NoCompatibilityEntry";
  value.installations[0].compatibility.message = "版本未命中已验证范围";
  value.installations[0].compatibility.allowed_actions = ["run_read_only_preflight"];
  return value;
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

describe("desktop station navigation", () => {
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
          serve: {
            phase: "running",
            running: true,
            listen: "127.0.0.1:8787",
            virtual_key: "vk-overlap",
            error: null,
          },
        });
      }
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") {
        scans += 1;
        if (scans === 1) return [scannedClaude];
        return scans === 2 ? slow : newestScan;
      }
      if (command === "plan_agent_connection") {
        return { operation_id: "op-overlap", confirmation_token: "token-overlap" };
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
    expect(await screen.findByRole("heading", { name: "用量" })).toBeInTheDocument();
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
      serve: { phase: "running", running: true, listen: "127.0.0.1:8787", virtual_key: "vk-test-secret", error: null },
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

  it("opens Add Provider as a separate page and returns to the source page after saving", async () => {
    const user = userEvent.setup();
    invokeMock.mockImplementation(async (command) => {
      if (["get_state", "add_provider"].includes(command)) return stateFixture();
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

  it("performs a safe Agent connection in one click without a confirmation dialog", async () => {
    const user = userEvent.setup();
    const running = stateFixture({ serve: { phase: "running", running: true, listen: "127.0.0.1:8787", virtual_key: "vk-test", error: null } });
    let scans = 0;
    invokeMock.mockImplementation(async (command) => {
      if (command === "get_state") return running;
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") { scans += 1; return [scannedClaude]; }
      if (command === "plan_agent_connection") return { operation_id: "op-1", confirmation_token: "token-1" };
      if (command === "apply_agent_plan") return { operation_id: "op-1", maintenance_warning: null };
      throw new Error(`unexpected IPC command: ${command}`);
    });
    render(<App />);
    await waitFor(() => expect(scans).toBe(1));
    await user.click(navigation().getByRole("button", { name: "Claude Code" }));
    expect(screen.queryByText("/opt/claude")).toBeNull();
    expect(screen.queryByRole("button", { name: /选择安装/ })).toBeNull();
    await user.click(await screen.findByRole("button", { name: "一键接入" }));
    expect(invokeMock).toHaveBeenCalledWith("plan_agent_connection", {
      agentId: "claude-code",
      installationPath: "/opt/claude",
    });
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("apply_agent_plan", { operationId: "op-1", confirmationToken: "token-1" }));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(await screen.findByText("Agent 已接入，无需再次确认")).toBeInTheDocument();
    expect(scans).toBe(2);
  });

  it("connects an unverified-but-preflight-passed version in one click, no dialog", async () => {
    // cc-switch-style: no scary modal. The version note is shown inline; clicking
    // One-click connection runs the preflight-approved plan and confirms automatically. The backend still
    // snapshots + rolls back). This is the P1 relaxation the user chose.
    const user = userEvent.setup();
    const running = stateFixture({ serve: { phase: "running", running: true, listen: "127.0.0.1:8787", virtual_key: "vk-test", error: null } });
    const unknown = experimentalClaude();
    invokeMock.mockImplementation(async (command) => {
      if (command === "get_state") return running;
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return [unknown];
      if (command === "plan_agent_connection") return { operation_id: "op-experimental", confirmation_token: "token-experimental" };
      if (command === "apply_agent_plan") return { operation_id: "op-experimental", maintenance_warning: null };
      throw new Error(`unexpected IPC command: ${command}`);
    });

    render(<App />);
    await screen.findByLabelText("主导航");
    await user.click(navigation().getByRole("button", { name: "Claude Code" }));
    // Honest inline note replaces the modal; the click is still informed.
    expect(await screen.findByText(/一键接入会先创建快照/)).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "一键接入" }));
    // One click drives the whole preflight-gated, auto-confirmed pipeline.
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("plan_agent_connection", {
      agentId: "claude-code",
      installationPath: "/opt/claude",
      expectedVersion: "2.1.210",
    }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("apply_agent_plan", {
      operationId: "op-experimental",
      confirmationToken: "token-experimental",
      experimentalCompatibilityConfirmed: true,
    }));
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });

  it("selects among multiple installations without displaying full paths", async () => {
    const user = userEvent.setup();
    const secondInstallation = structuredClone(scannedClaude.installations[0]);
    secondInstallation.discovery.executable_path = "/Users/x/.local/lib/node_modules/@anthropic-ai/claude-code/bin/claude.exe";
    secondInstallation.discovery.canonical_path = "/Users/x/.local/lib/node_modules/@anthropic-ai/claude-code/bin/claude.exe";
    secondInstallation.discovery.version_raw = "10.0.0";
    secondInstallation.discovery.version_normalized = "10.0.0";
    secondInstallation.compatibility.installation_path = secondInstallation.discovery.canonical_path;
    const multipleClaude: AgentView = {
      ...scannedClaude,
      status: "MULTIPLE_INSTALLATIONS",
      installations: [scannedClaude.installations[0], secondInstallation],
    };
    const running = stateFixture({ serve: { phase: "running", running: true, listen: "127.0.0.1:8787", virtual_key: "vk-test", error: null } });
    invokeMock.mockImplementation(async (command) => {
      if (command === "get_state") return running;
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return [multipleClaude];
      if (command === "plan_agent_connection") return { operation_id: "op-2", confirmation_token: "token-2" };
      if (command === "apply_agent_plan") return { operation_id: "op-2", maintenance_warning: null };
      throw new Error(`unexpected IPC command: ${command}`);
    });

    render(<App />);
    const nav = within(await screen.findByLabelText("主导航"));
    await user.click(nav.getByRole("button", { name: "Claude Code" }));
    expect(document.body).not.toHaveTextContent(secondInstallation.discovery.canonical_path);
    await user.click(await screen.findByRole("button", { name: /选择安装/ }));
    await user.click(screen.getByRole("option", { name: "claude.exe · v10.0.0" }));
    expect(screen.queryByRole("listbox")).toBeNull();
    expect(document.body).not.toHaveTextContent(secondInstallation.discovery.canonical_path);
    await user.click(screen.getByRole("button", { name: "一键接入" }));
    expect(invokeMock).toHaveBeenCalledWith("plan_agent_connection", {
      agentId: "claude-code",
      installationPath: secondInstallation.discovery.canonical_path,
    });
  });
});
