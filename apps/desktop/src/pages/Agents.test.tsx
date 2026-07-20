import { invoke } from "@tauri-apps/api/core";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type {
  AgentStatus,
  AgentView,
  ConfigPlanView,
  SnapshotView,
} from "../api";
import Agents from "./Agents";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const invokeMock = vi.mocked(invoke);

function agent(
  id: string,
  name: string,
  status: AgentStatus,
  options: { connected?: boolean; paths?: string[]; detected?: boolean } = {},
): AgentView {
  const detected = options.detected ?? true;
  const paths = options.paths ?? [`/opt/${id}`];
  return {
    metadata: {
      agent_id: id,
      legacy_kind: null,
      display_name: name,
      icon_key: id,
      admission: "supported",
    },
    installations: detected
      ? paths.map((path) => ({
          connected: options.connected ?? false,
          discovery: {
            agent_id: id,
            executable_path: path,
            canonical_path: path,
            version_raw: "1.2.3",
            version_normalized: "1.2.3",
            environment: "macos",
            evidence: [{ source: "path", observed_path: path, is_path_default: true }],
            is_path_default: true,
            runnable: true,
            config_candidates: [`/tmp/${id}.json`],
            config_fingerprint: null,
            conflict_group: paths.length > 1 ? "multi" : null,
            diagnostics: [],
            scanned_at_ms: 1,
          },
          compatibility: {
            agent_id: id,
            installation_path: path,
            status,
            reason_code: status === "DETECTED_BLOCKED" ? "BlockedVersionMatch" : "VerifiedRangeMatch",
            message:
              status === "DETECTED_BLOCKED"
                ? "该版本命中阻断规则"
                : status === "DETECTED_UNKNOWN"
                  ? "当前目录没有该版本"
                  : "版本命中已验证兼容范围",
            matched_catalog_version: "fixture",
            connector_id: status === "DETECTED_BLOCKED" ? null : `${id}-v1`,
            allowed_actions: status === "DETECTED_VERIFIED" ? ["preview_connect"] : [],
          },
        }))
      : [],
    status: options.connected ? "CONNECTED" : detected ? status : "NOT_DETECTED",
    catalog_sequence: 7,
    catalog_expires_at_ms: null,
    catalog_source: "builtin",
    catalog_warning: null,
  };
}

const fiveAgents = (): AgentView[] => [
  agent("claude-code", "Claude Code", "DETECTED_VERIFIED"),
  agent("codex", "Codex", "DETECTED_UNKNOWN"),
  agent("opencode", "OpenCode", "DETECTED_BLOCKED"),
  agent("openclaw", "OpenClaw", "DETECTED_UNKNOWN"),
  agent("nous-hermes-agent", "Hermes Agent", "DETECTED_UNKNOWN", { detected: false }),
];

function plan(intent: "connect" | "disconnect" | "restore", expires = Date.now() + 300_000): ConfigPlanView {
  return {
    schema_version: 1,
    operation_id: "ab".repeat(16),
    intent,
    agent_id: "claude-code",
    installation_path: "/opt/claude-code",
    target_config_path: "/tmp/settings.json",
    target_existed: true,
    before_hash: "01".repeat(32),
    expected_after_hash: "02".repeat(32),
    owned_paths: [{ segments: ["env", "ANTHROPIC_AUTH_TOKEN"] }],
    changes: [
      {
        operation: "replace",
        path: { segments: ["env", "ANTHROPIC_AUTH_TOKEN"] },
        sensitive: true,
        summary: "<敏感值已隐藏>",
      },
    ],
    human_diff: "~ /env/ANTHROPIC_AUTH_TOKEN: <敏感值已隐藏>",
    connector_id: "claude-code-v1",
    compatibility_evidence: {
      agent_id: "claude-code",
      installation_path: "/opt/claude-code",
      status: "DETECTED_VERIFIED",
      reason_code: "VerifiedRangeMatch",
      message: "版本命中已验证兼容范围",
      matched_catalog_version: "fixture",
      connector_id: "claude-code-v1",
      allowed_actions: ["preview_connect"],
    },
    compatibility_sequence: 7,
    compatibility_expires_at_ms: null,
    created_at_ms: Date.now(),
    expires_at_ms: expires,
    required_confirmations: ["installation", "target_config", "configuration_diff"],
    confirmation_token: "cd".repeat(32),
  };
}

const snapshot: SnapshotView = {
  snapshot_id: "ef".repeat(16),
  agent_id: "claude-code",
  target_config_path: "/tmp/settings.json",
  created_at_ms: Date.now(),
  connector_id: "claude-code-v1",
  app_version: "0.1.0",
  original_existed: true,
  pinned: true,
  source: "encrypted",
  restorable: true,
};

let scans: AgentView[];
let planned: ConfigPlanView;
let snapshotRows: SnapshotView[];

beforeEach(() => {
  scans = fiveAgents();
  planned = plan("connect");
  snapshotRows = [snapshot];
  invokeMock.mockReset();
  invokeMock.mockImplementation(async (command) => {
    if (command === "scan_agents") return scans;
    if (command === "plan_agent_connection" || command === "plan_agent_disconnect") return planned;
    if (command === "list_agent_snapshots") return snapshotRows;
    if (command === "plan_snapshot_restore") return planned;
    if (command === "apply_agent_plan" || command === "apply_snapshot_restore") {
      return {
        operation_id: planned.operation_id,
        agent_id: planned.agent_id,
        target_config_path: planned.target_config_path,
        before_hash: planned.before_hash,
        after_hash: planned.expected_after_hash,
        snapshot_id: snapshot.snapshot_id,
        ownership_revision: 1,
        maintenance_warning: null,
      };
    }
    throw new Error(`unexpected IPC command: ${command}`);
  });
});

async function renderReady(serveRunning = true) {
  const result = render(<Agents serveRunning={serveRunning} />);
  await screen.findByText("Claude Code");
  return result;
}

async function confirmEveryCheckbox(user: ReturnType<typeof userEvent.setup>) {
  for (const checkbox of screen.getAllByRole("checkbox")) await user.click(checkbox);
}

async function expandAgent(user: ReturnType<typeof userEvent.setup>, id: string) {
  const row = screen.getByTestId(`agent-${id}`);
  const toggle = within(row).getByRole("button");
  if (toggle.getAttribute("aria-expanded") !== "true") await user.click(toggle);
  return row;
}

describe("Agents page", () => {
  it("uses a compact list and keeps only one agent row expanded", async () => {
    const user = userEvent.setup();
    await renderReady();

    expect(screen.getByRole("heading", { name: "Agent 管理" })).toBeInTheDocument();
    expect(screen.getByRole("list", { name: "Agent 列表" })).toBeInTheDocument();

    const claude = screen.getByTestId("agent-claude-code");
    const codex = screen.getByTestId("agent-codex");
    const claudeToggle = within(claude).getByRole("button", { name: /Claude Code/ });
    const codexToggle = within(codex).getByRole("button", { name: /Codex/ });

    expect(claudeToggle).toHaveAttribute("aria-expanded", "false");
    expect(within(claude).queryByRole("button", { name: "预览接入" })).not.toBeInTheDocument();

    await user.click(claudeToggle);
    expect(claudeToggle).toHaveAttribute("aria-expanded", "true");
    expect(within(claude).getByRole("button", { name: "预览接入" })).toBeInTheDocument();

    await user.click(codexToggle);
    expect(claudeToggle).toHaveAttribute("aria-expanded", "false");
    expect(codexToggle).toHaveAttribute("aria-expanded", "true");
    expect(within(claude).queryByRole("button", { name: "预览接入" })).not.toBeInTheDocument();
  });

  it("renders all five Registry agents and disables unknown, blocked and discovery-only actions", async () => {
    const user = userEvent.setup();
    await renderReady();
    for (const name of ["Claude Code", "Codex", "OpenCode", "OpenClaw", "Hermes Agent"]) {
      expect(screen.getByText(name)).toBeInTheDocument();
    }
    expect(within(await expandAgent(user, "claude-code")).getByRole("button", { name: "预览接入" })).toBeEnabled();
    for (const id of ["codex", "opencode", "openclaw", "nous-hermes-agent"]) {
      expect(within(await expandAgent(user, id)).getByRole("button", { name: "预览接入" })).toBeDisabled();
    }
  });

  it("shows loading, empty and scan error states without affecting the rest of the app", async () => {
    let resolveScan!: (value: AgentView[]) => void;
    invokeMock.mockImplementationOnce(
      () => new Promise<AgentView[]>((resolve) => { resolveScan = resolve; }),
    );
    const { unmount } = render(<Agents serveRunning />);
    expect(screen.getByText("正在只读扫描本机 Agent…")).toBeInTheDocument();
    resolveScan([]);
    await screen.findByText("Registry 没有可展示的 Agent。");
    unmount();

    invokeMock.mockRejectedValueOnce({ message: "扫描失败", code: "scan_failed" });
    render(<Agents serveRunning />);
    await screen.findByText(/扫描失败.*scan_failed/);
  });

  it("requires an exact multi-install selection before requesting a plan", async () => {
    const user = userEvent.setup();
    scans = [agent("claude-code", "Claude Code", "MULTIPLE_INSTALLATIONS", { paths: ["/opt/claude-a", "/opt/claude-b"] })];
    await renderReady();
    const card = await expandAgent(user, "claude-code");
    expect(within(card).getByRole("button", { name: "预览接入" })).toBeDisabled();
    await user.selectOptions(within(card).getByLabelText("Claude Code 安装实例"), "/opt/claude-b");
    await user.click(within(card).getByRole("button", { name: "预览接入" }));
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("plan_agent_connection", {
        agentId: "claude-code",
        installationPath: "/opt/claude-b",
      }),
    );
  });

  it("never applies before the diff is shown and every confirmation is checked; cancel is zero apply", async () => {
    const user = userEvent.setup();
    await renderReady();
    await user.click(within(await expandAgent(user, "claude-code")).getByRole("button", { name: "预览接入" }));
    expect(await screen.findByText(/ANTHROPIC_AUTH_TOKEN/)).toBeInTheDocument();
    const apply = screen.getByRole("button", { name: "确认并接入" });
    expect(apply).toBeDisabled();
    expect(invokeMock.mock.calls.some(([command]) => command === "apply_agent_plan")).toBe(false);
    await user.click(screen.getByRole("button", { name: "取消" }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(invokeMock.mock.calls.some(([command]) => command === "apply_agent_plan")).toBe(false);
  });

  it("requires re-preview after expiry and successfully refreshes after a confirmed apply", async () => {
    const user = userEvent.setup();
    planned = plan("connect", Date.now() - 1);
    await renderReady();
    const claude = await expandAgent(user, "claude-code");
    await user.click(within(claude).getByRole("button", { name: "预览接入" }));
    expect(await screen.findByText("计划已过期")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "确认并接入" })).toBeDisabled();
    await user.click(screen.getByRole("button", { name: "取消" }));

    planned = plan("connect");
    await user.click(within(claude).getByRole("button", { name: "预览接入" }));
    await screen.findByRole("dialog");
    await confirmEveryCheckbox(user);
    await user.click(screen.getByRole("button", { name: "确认并接入" }));
    await screen.findByText("Agent 已接入");
    expect(invokeMock.mock.calls.filter(([command]) => command === "scan_agents").length).toBeGreaterThan(1);
  });

  it("puts disconnect and snapshot restore behind the same second-confirmation boundary", async () => {
    const user = userEvent.setup();
    scans = [agent("claude-code", "Claude Code", "DETECTED_VERIFIED", { connected: true })];
    planned = plan("disconnect");
    await renderReady();
    const claude = await expandAgent(user, "claude-code");
    await user.click(within(claude).getByRole("button", { name: "预览断开" }));
    expect(await screen.findByRole("button", { name: "确认并断开" })).toBeDisabled();
    await confirmEveryCheckbox(user);
    await user.click(screen.getByRole("button", { name: "确认并断开" }));
    await waitFor(() => expect(invokeMock.mock.calls.some(([command]) => command === "apply_agent_plan")).toBe(true));

    planned = plan("restore");
    await user.click(within(claude).getByRole("button", { name: "查看快照" }));
    await user.click(await screen.findByRole("button", { name: "预览恢复" }));
    expect(await screen.findByRole("button", { name: "确认并恢复" })).toBeDisabled();
    await confirmEveryCheckbox(user);
    await user.click(screen.getByRole("button", { name: "确认并恢复" }));
    await waitFor(() => expect(invokeMock.mock.calls.some(([command]) => command === "apply_snapshot_restore")).toBe(true));
  });
});
