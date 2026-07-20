import { invoke } from "@tauri-apps/api/core";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import App from "./App";
import type { AgentUiMetadataView, AgentView, StateView } from "./api";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const invokeMock = vi.mocked(invoke);

const stateFixture: StateView = {
  providers: [],
  tiers: {
    high: { upstream: null, model: null },
    mid: { upstream: null, model: null },
    low: { upstream: null, model: null },
  },
  serve: { running: false, listen: "127.0.0.1:8787", virtual_key: null },
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
};

const registryFixture: AgentUiMetadataView[] = [
  { agent_id: "claude-code", legacy_kind: "cc", display_name: "Claude Code", icon_key: "claude", admission: "supported" },
  { agent_id: "codex", legacy_kind: "codex", display_name: "Codex", icon_key: "codex", admission: "supported" },
  { agent_id: "opencode", legacy_kind: "opencode", display_name: "OpenCode", icon_key: "opencode", admission: "supported" },
  { agent_id: "openclaw", legacy_kind: null, display_name: "OpenClaw", icon_key: "openclaw", admission: "supported" },
  { agent_id: "nous-hermes-agent", legacy_kind: null, display_name: "Hermes Agent", icon_key: "hermes", admission: "supported" },
  { agent_id: "future-agent", legacy_kind: null, display_name: "Future Agent", icon_key: "future", admission: "discovery_only" },
];

const scannedAgentFixture: AgentView = {
  metadata: registryFixture[0],
  installations: [
    {
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
    },
  ],
  status: "DETECTED_VERIFIED",
  catalog_sequence: 1,
  catalog_expires_at_ms: null,
  catalog_source: "builtin",
  catalog_warning: null,
};

beforeEach(() => {
  invokeMock.mockImplementation(async (command) => {
    if (command === "get_state") return stateFixture;
    if (command === "list_agent_registry") return registryFixture;
    if (command === "scan_agents") return [];
    throw new Error(`unexpected IPC command: ${command}`);
  });
});

describe("dynamic Agent navigation", () => {
  it("scans once per App process, reuses the result across navigation, and rescans only on request", async () => {
    const user = userEvent.setup();
    const { container } = render(<App />);
    await screen.findByText("token-station");
    expect(container.querySelector(".agentbar")).toBeNull();

    await waitFor(() =>
      expect(invokeMock.mock.calls.filter(([command]) => command === "scan_agents")).toHaveLength(1),
    );

    await user.click(screen.getByRole("button", { name: "Agents" }));
    await screen.findByRole("heading", { name: "Agent 管理" });
    expect(screen.getAllByRole("listitem")).toHaveLength(registryFixture.length);
    await user.click(screen.getByRole("button", { name: "主页" }));
    await user.click(screen.getByRole("button", { name: "Agents" }));
    expect(invokeMock.mock.calls.filter(([command]) => command === "scan_agents")).toHaveLength(1);

    await user.click(screen.getByRole("button", { name: "重新扫描" }));
    await waitFor(() =>
      expect(invokeMock.mock.calls.filter(([command]) => command === "scan_agents")).toHaveLength(2),
    );
  });

  it("starts and stops the proxy and saves the editable configuration", async () => {
    const running = {
      ...stateFixture,
      serve: { running: true, listen: "127.0.0.1:8787", virtual_key: "vk-test" },
    } satisfies StateView;
    invokeMock.mockImplementation(async (command) => {
      if (command === "get_state" || command === "serve_stop" || command === "save_config") {
        return stateFixture;
      }
      if (command === "list_agent_registry") return registryFixture;
      if (command === "serve_start") return running;
      if (command === "scan_agents") return [];
      throw new Error(`unexpected IPC command: ${command}`);
    });
    const user = userEvent.setup();
    render(<App />);
    await user.click(await screen.findByRole("button", { name: "启动代理" }));
    expect(await screen.findByText("vk-test")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "停止" }));
    expect(await screen.findByRole("button", { name: "启动代理" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "保存并应用" }));
    expect(await screen.findByText("已保存并校验")).toBeInTheDocument();
  });

  it("keeps the last successful Agent result when a manual rescan fails", async () => {
    let scans = 0;
    invokeMock.mockImplementation(async (command) => {
      if (command === "get_state") return stateFixture;
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") {
        scans += 1;
        if (scans === 1) return [scannedAgentFixture];
        throw { message: "扫描失败", code: "scan_failed" };
      }
      throw new Error(`unexpected IPC command: ${command}`);
    });
    const user = userEvent.setup();
    render(<App />);
    await waitFor(() => expect(scans).toBe(1));
    await user.click(screen.getByRole("button", { name: "Agents" }));
    expect(await screen.findByText("9.9.9")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "重新扫描" }));

    expect(await screen.findByText(/扫描失败.*scan_failed/)).toBeInTheDocument();
    expect(screen.getByText("9.9.9")).toBeInTheDocument();
  });

  it("discovers preset models and submits only the structured provider form", async () => {
    invokeMock.mockImplementation(async (command) => {
      if (command === "get_state" || command === "add_provider") return stateFixture;
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return [];
      if (command === "discover_provider_models") {
        return { models: ["gpt-new"], source: "live", fetched_at_ms: 1, warning: null };
      }
      throw new Error(`unexpected IPC command: ${command}`);
    });
    const user = userEvent.setup();
    render(<App />);
    await screen.findByText("token-station");
    await user.selectOptions(screen.getAllByRole("combobox")[6], "openai");
    expect(screen.getByText("https://api.openai.com/v1")).toBeInTheDocument();
    await user.type(screen.getByPlaceholderText("API Key"), "secret-test");
    await user.click(screen.getByRole("button", { name: "刷新模型" }));
    expect(await screen.findByText("已同步 1 个")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "添加" }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("add_provider", {
      name: "openai",
      baseUrl: "https://api.openai.com/v1",
      models: ["gpt-5.5", "gpt-5.5-mini", "gpt-4.1", "o4-mini"],
      apiKey: "secret-test",
    }));
    expect(screen.getByText("供应商已添加")).toBeInTheDocument();
  });
});
