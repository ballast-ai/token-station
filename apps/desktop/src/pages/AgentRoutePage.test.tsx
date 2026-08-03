import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import AgentRoutePage from "./AgentRoutePage";
import {
  getAgentDrift,
  planAgentConnection,
  planAgentDisconnect,
  type AgentInstallationView,
  type AgentView,
} from "../api";

vi.mock("../api", () => ({
  applyAgentPlan: vi.fn(),
  getAgentDrift: vi.fn(),
  mountAgentProfile: vi.fn(),
  planAgentConnection: vi.fn(),
  planAgentDisconnect: vi.fn(),
  saveAgentRoutes: vi.fn(),
  setAgentRouteMode: vi.fn(),
  setAgentTier: vi.fn(),
}));

function installation(path: string, version: string): AgentInstallationView {
  return {
    managed: false,
    connected: false,
    adapter_ready: true,
    discovery: {
      agent_id: "claude-code",
      executable_path: path,
      canonical_path: path,
      binary_source: "path",
      modified_at_ms: null,
      binary_sha256: null,
      upgrade_command: null,
      version_raw: version,
      version_normalized: version,
      environment: "macos",
      evidence: [],
      is_path_default: false,
      runnable: true,
      config_candidates: ["/Users/x/.claude/settings.json"],
      config_fingerprint: null,
      conflict_group: "claude-multiple",
      diagnostics: [{ reason_code: "MULTIPLE_CANONICAL_PATHS", message: "multiple" }],
      scanned_at_ms: 1,
    },
    compatibility: {
      agent_id: "claude-code",
      installation_path: path,
      status: "MULTIPLE_INSTALLATIONS",
      reason_code: "MULTIPLE_CANONICAL_PATHS",
      message: "检测到多个安装实例，请先选择精确路径",
      matched_catalog_version: "builtin",
      connector_id: null,
      allowed_actions: ["select_installation"],
    },
  };
}

describe("AgentRoutePage multi-install admission", () => {
  beforeEach(() => {
    vi.mocked(getAgentDrift).mockReset().mockResolvedValue([]);
    vi.mocked(planAgentConnection).mockReset().mockReturnValue(new Promise(() => undefined));
    vi.mocked(planAgentDisconnect).mockReset().mockReturnValue(new Promise(() => undefined));
  });

  it("lets the user connect the exact Claude Code installation they selected", async () => {
    const user = userEvent.setup();
    const installations = [
      installation("/Users/x/.local/bin/claude", "2.1.211"),
      installation("/Users/x/.nvm/bin/claude", "2.1.170"),
    ];
    const agent: AgentView = {
      metadata: {
        agent_id: "claude-code",
        legacy_kind: "cc",
        display_name: "Claude Code",
        icon_key: "claude",
        admission: "supported",
        ui_order: 10,
        nav_mark: "C",
      },
      installations,
      status: "MULTIPLE_INSTALLATIONS",
      catalog_sequence: 1,
      catalog_expires_at_ms: null,
      catalog_source: "builtin",
      catalog_warning: null,
    };

    render(
      <AgentRoutePage
        metadata={agent.metadata}
        agent={agent}
        route={{
          mode: "inherit",
          tiers: {
            high: { upstream: null, model: null },
            mid: { upstream: null, model: null },
            low: { upstream: null, model: null },
          },
          config_error: null,
          profile: null,
          routing_mode: "tiered",
        }}
        providers={[]}
        profiles={[]}
        quotaAccounts={[]}
        serveRunning
        applying={false}
        onStateChange={vi.fn()}
        onRescan={vi.fn()}
        onSaveQuota={vi.fn()}
        onSaveQuotaPlan={vi.fn()}
        onViewQuotaUsage={vi.fn()}
      />,
    );

    const connect = screen.getByRole("button", { name: "一键接入" });
    expect(connect).toBeDisabled();
    await user.click(screen.getByRole("button", { name: /选择安装/ }));
    await user.click(screen.getByRole("option", { name: "claude · v2.1.211" }));

    expect(connect).toBeEnabled();
    expect(screen.getByText("可接入")).toBeInTheDocument();
    await user.click(connect);
    await waitFor(() => expect(planAgentConnection).toHaveBeenCalledWith(
      "claude-code",
      "/Users/x/.local/bin/claude",
      { expectedVersion: "2.1.211" },
    ));
  });

  it("offers recovery instead of another connect plan when ownership exists but runtime validation fails", async () => {
    const user = userEvent.setup();
    const owned = installation("/opt/homebrew/bin/opencode", "1.18.2");
    owned.managed = true;
    owned.discovery.agent_id = "opencode";
    owned.discovery.is_path_default = true;
    owned.discovery.conflict_group = null;
    owned.discovery.diagnostics = [];
    owned.compatibility = {
      agent_id: "opencode",
      installation_path: owned.discovery.canonical_path,
      status: "DETECTED_VERIFIED",
      reason_code: "DEFAULT_ADMISSION",
      message: "已发现兼容安装",
      matched_catalog_version: "builtin",
      connector_id: "opencode-v1",
      allowed_actions: ["preview_connect"],
    };
    const agent: AgentView = {
      metadata: {
        agent_id: "opencode",
        legacy_kind: "opencode",
        display_name: "OpenCode",
        icon_key: "opencode",
        admission: "supported",
        ui_order: 50,
        nav_mark: "O",
      },
      installations: [owned],
      status: "DETECTED_VERIFIED",
      catalog_sequence: 1,
      catalog_expires_at_ms: null,
      catalog_source: "builtin",
      catalog_warning: null,
    };

    render(
      <AgentRoutePage
        metadata={agent.metadata}
        agent={agent}
        route={{
          mode: "inherit",
          tiers: {
            high: { upstream: null, model: null },
            mid: { upstream: null, model: null },
            low: { upstream: null, model: null },
          },
          config_error: null,
          profile: null,
          routing_mode: "tiered",
        }}
        providers={[]}
        profiles={[]}
        quotaAccounts={[]}
        serveRunning
        applying={false}
        onStateChange={vi.fn()}
        onRescan={vi.fn()}
        onSaveQuota={vi.fn()}
        onSaveQuotaPlan={vi.fn()}
        onViewQuotaUsage={vi.fn()}
      />,
    );

    expect(screen.getByText("需修复")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "恢复 Agent 原始配置" }));

    await waitFor(() => expect(planAgentDisconnect).toHaveBeenCalledWith(
      "opencode",
      "/opt/homebrew/bin/opencode",
    ));
    expect(planAgentConnection).not.toHaveBeenCalled();
  });

  it("disables a new connection when the running Gateway skipped the required adapter", async () => {
    const skipped = installation("/opt/homebrew/bin/claude", "2.1.211");
    skipped.adapter_ready = false;
    skipped.discovery.is_path_default = true;
    skipped.discovery.conflict_group = null;
    skipped.discovery.diagnostics = [];
    skipped.compatibility = {
      agent_id: "claude-code",
      installation_path: skipped.discovery.canonical_path,
      status: "DETECTED_VERIFIED",
      reason_code: "DEFAULT_ADMISSION",
      message: "已发现兼容安装",
      matched_catalog_version: "builtin",
      connector_id: "claude-code-v1",
      allowed_actions: ["preview_connect"],
    };
    const agent: AgentView = {
      metadata: {
        agent_id: "claude-code",
        legacy_kind: "cc",
        display_name: "Claude Code",
        icon_key: "claude",
        admission: "supported",
        ui_order: 10,
        nav_mark: "C",
      },
      installations: [skipped],
      status: "DETECTED_VERIFIED",
      catalog_sequence: 1,
      catalog_expires_at_ms: null,
      catalog_source: "builtin",
      catalog_warning: null,
    };

    render(
      <AgentRoutePage
        metadata={agent.metadata}
        agent={agent}
        route={{
          mode: "inherit",
          tiers: {
            high: { upstream: null, model: null },
            mid: { upstream: null, model: null },
            low: { upstream: null, model: null },
          },
          config_error: null,
          profile: null,
          routing_mode: "tiered",
        }}
        providers={[]}
        profiles={[]}
        quotaAccounts={[]}
        serveRunning
        applying={false}
        onStateChange={vi.fn()}
        onRescan={vi.fn()}
        onSaveQuota={vi.fn()}
        onSaveQuotaPlan={vi.fn()}
        onViewQuotaUsage={vi.fn()}
      />,
    );

    expect(await screen.findByText("适配器未就绪")).toBeInTheDocument();
    expect(screen.getByText(/Agent 配置未被修改/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "一键接入" })).toBeDisabled();
    expect(planAgentConnection).not.toHaveBeenCalled();
  });
});
