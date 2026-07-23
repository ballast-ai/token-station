import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import AgentRoutePage from "./AgentRoutePage";
import {
  getAgentDrift,
  planAgentConnection,
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
    connected: false,
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
        }}
        providers={[]}
        profiles={[]}
        serveRunning
        onStateChange={vi.fn()}
        onRescan={vi.fn()}
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
});
