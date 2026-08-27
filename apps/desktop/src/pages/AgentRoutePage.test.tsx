import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import AgentRoutePage, { compactDiscoveryPath } from "./AgentRoutePage";
import { ErrorToastProvider } from "../components/ErrorToast";
import {
  applyAgentPlan,
  configureCursorProvider,
  discardAgentPlan,
  ensureServeRunning,
  forceForgetAgent,
  getAgentBackupDirectory,
  getAgentDrift,
  getCursorProviderStatus,
  mountAgentProfile,
  openAgentBackupDirectory,
  planAgentConnection,
  planAgentDisconnect,
  revealAgentPlanSensitiveValues,
  restartAgentRoute,
  restoreCursorProvider,
  saveAgentRoutes,
  setAgentRouteMode,
  type AgentInstallationView,
  type AgentView,
} from "../api";

vi.mock("../api", () => ({
  applyAgentPlan: vi.fn(),
  configureCursorProvider: vi.fn(),
  discardAgentPlan: vi.fn(),
  ensureServeRunning: vi.fn(),
  forceForgetAgent: vi.fn(),
  getAgentBackupDirectory: vi.fn(),
  getAgentDrift: vi.fn(),
  getCursorProviderStatus: vi.fn(),
  mountAgentProfile: vi.fn(),
  openAgentBackupDirectory: vi.fn(),
  planAgentConnection: vi.fn(),
  planAgentDisconnect: vi.fn(),
  revealAgentPlanSensitiveValues: vi.fn(),
  restartAgentRoute: vi.fn(),
  restoreCursorProvider: vi.fn(),
  saveAgentRoutes: vi.fn(),
  setAgentRouteMode: vi.fn(),
  setAgentTier: vi.fn(),
}));

afterEach(() => vi.useRealTimers());

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
    vi.mocked(ensureServeRunning).mockReset().mockResolvedValue({} as never);
    vi.mocked(applyAgentPlan).mockReset().mockResolvedValue({} as never);
    vi.mocked(configureCursorProvider).mockReset().mockResolvedValue({
      state: "connected",
      message: "Cursor 已配置",
    });
    vi.mocked(discardAgentPlan).mockReset().mockResolvedValue(undefined);
    vi.mocked(getAgentDrift).mockReset().mockResolvedValue([]);
    vi.mocked(getCursorProviderStatus).mockReset().mockResolvedValue({
      state: "disconnected",
      message: null,
    });
    vi.mocked(planAgentConnection).mockReset().mockReturnValue(new Promise(() => undefined));
    vi.mocked(planAgentDisconnect).mockReset().mockReturnValue(new Promise(() => undefined));
    vi.mocked(revealAgentPlanSensitiveValues).mockReset().mockResolvedValue([]);
    vi.mocked(forceForgetAgent).mockReset().mockReturnValue(new Promise(() => undefined));
    vi.mocked(getAgentBackupDirectory).mockReset().mockResolvedValue("/Users/x/Library/Application Support/com.tokenstation.desktop/agent-integration/snapshots");
    vi.mocked(mountAgentProfile).mockReset().mockResolvedValue({} as never);
    vi.mocked(openAgentBackupDirectory).mockReset().mockResolvedValue("/Users/x/Library/Application Support/com.tokenstation.desktop/agent-integration/snapshots");
    vi.mocked(restartAgentRoute).mockReset().mockResolvedValue({} as never);
    vi.mocked(restoreCursorProvider).mockReset().mockResolvedValue({
      state: "disconnected",
      message: "已恢复 Cursor 官方配置并断开",
    });
    vi.mocked(saveAgentRoutes).mockReset().mockResolvedValue({} as never);
    vi.mocked(setAgentRouteMode).mockReset().mockResolvedValue({} as never);
  });

  it("shows the Agent icon beside the routing page name", () => {
    render(
      <AgentRoutePage
        metadata={{
          agent_id: "claude-code",
          legacy_kind: "cc",
          display_name: "Claude Code",
          icon_key: "claude",
          admission: "supported",
        }}
        route={{
          mode: "inherit",
          tiers: {
            high: { upstream: null, model: null },
            mid: { upstream: null, model: null },
            low: { upstream: null, model: null },
          },
          config_error: null,
          profile: null,
          routing_mode: "direct",
        }}
        providers={[]}
        profiles={[]}
        quotaAccounts={[]}
        serveRunning={false}
        applying={false}
        onStateChange={vi.fn()}
        onRefreshAgents={vi.fn()}
        onSaveQuota={vi.fn()}
        onSaveQuotaPlan={vi.fn()}
        onViewQuotaUsage={vi.fn()}
        pageMode="routing"
      />,
    );

    const heading = screen.getByRole("heading", { name: "Claude Code" }).closest("header");
    expect(heading?.querySelector('[data-agent-brand="claude-code"]')).toBeInTheDocument();
  });

  it("shows the exact encrypted backup directory and can open it without sending a renderer path", async () => {
    const user = userEvent.setup();
    render(
      <AgentRoutePage
        metadata={{
          agent_id: "claude-code",
          legacy_kind: "cc",
          display_name: "Claude Code",
          icon_key: "claude",
          admission: "supported",
        }}
        route={{
          mode: "inherit",
          tiers: {
            high: { upstream: null, model: null },
            mid: { upstream: null, model: null },
            low: { upstream: null, model: null },
          },
          config_error: null,
          profile: null,
          routing_mode: "direct",
        }}
        providers={[]}
        profiles={[]}
        quotaAccounts={[]}
        serveRunning={false}
        applying={false}
        onStateChange={vi.fn()}
        onRefreshAgents={vi.fn()}
        onSaveQuota={vi.fn()}
        onSaveQuotaPlan={vi.fn()}
        onViewQuotaUsage={vi.fn()}
        pageMode="connection"
      />,
    );

    expect(await screen.findByText("/Users/x/Library/Application Support/com.tokenstation.desktop/agent-integration/snapshots")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "打开备份文件夹" }));
    expect(openAgentBackupDirectory).toHaveBeenCalledWith();
  });

  it("接入前展示字段级前后预览，确认后才按 ensure → plan → apply → cached 执行", async () => {
    const user = userEvent.setup();
    const found = installation("/opt/homebrew/bin/claude", "2.1.211");
    found.discovery.is_path_default = true;
    found.discovery.conflict_group = null;
    found.discovery.diagnostics = [];
    found.compatibility = {
      ...found.compatibility,
      status: "DETECTED_VERIFIED",
      reason_code: "DEFAULT_ADMISSION",
      connector_id: "claude-code-v1",
    };
    const agent: AgentView = {
      metadata: {
        agent_id: "claude-code",
        legacy_kind: "cc",
        display_name: "Claude Code",
        icon_key: "claude",
        admission: "supported",
      },
      installations: [found],
      status: "DETECTED_VERIFIED",
      catalog_sequence: 1,
      catalog_expires_at_ms: null,
      catalog_source: "builtin",
      catalog_warning: null,
    };
    vi.mocked(planAgentConnection).mockResolvedValue({
      operation_id: "operation-1",
      confirmation_token: "confirmation-1",
      target_config_path: "/Users/x/.claude/settings.json",
      changes: [{
        operation: "replace",
        path: { segments: ["env", "ANTHROPIC_BASE_URL"] },
        sensitive: false,
        summary: "<设置受管值>",
        before_preview: '"https://api.anthropic.com"',
        after_preview: '"http://127.0.0.1:8787/agents/claude-code/v1"',
      }, {
        operation: "replace",
        path: { segments: ["env", "ANTHROPIC_AUTH_TOKEN"] },
        sensitive: true,
        summary: "<敏感值已隐藏>",
        before_preview: '"must-not-render-old-secret"',
        after_preview: '"must-not-render-new-secret"',
      }, {
        operation: "add",
        path: { segments: ["env", "MAX_THINKING_TOKENS"] },
        sensitive: false,
        summary: "<设置受管值>",
        after_preview: '"0"',
      }, {
        operation: "add",
        path: { segments: ["env", "CLAUDE_CODE_DISABLE_ADAPTIVE_THINKING"] },
        sensitive: false,
        summary: "<设置受管值>",
        after_preview: '"1"',
      }],
      human_diff: "endpoint changed",
    } as never);
    const onRefreshAgents = vi.fn().mockResolvedValue(undefined);

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
        serveRunning={false}
        applying={false}
        onStateChange={vi.fn()}
        onRefreshAgents={onRefreshAgents}
        onSaveQuota={vi.fn()}
        onSaveQuotaPlan={vi.fn()}
        onViewQuotaUsage={vi.fn()}
      />,
    );

    const connect = screen.getByRole("button", { name: "预览并接入" });
    expect(connect).toBeEnabled();
    expect(screen.queryByText("点击一键接入后会自动启动代理，并等待代理可达。"))
      .not.toBeInTheDocument();
    await user.click(connect);

    const preview = await screen.findByRole("dialog", { name: "确认接入改动" });
    expect(within(preview).getByRole("region", { name: "配置改动" })).toHaveClass("agent-change-scroll");
    expect(preview).toHaveTextContent("/Users/x/.claude/settings.json");
    expect(preview).toHaveTextContent("env.ANTHROPIC_BASE_URL");
    expect(preview).toHaveTextContent("修改前");
    expect(preview).toHaveTextContent("修改后");
    expect(preview).toHaveTextContent('"https://api.anthropic.com"');
    expect(preview).toHaveTextContent('"http://127.0.0.1:8787/agents/claude-code/v1"');
    expect(preview).toHaveTextContent("env.MAX_THINKING_TOKENS");
    expect(preview).toHaveTextContent("未设置");
    expect(preview).toHaveTextContent('"0"');
    expect(preview).toHaveTextContent("Thinking token 预算设为 0");
    expect(preview).toHaveTextContent("关闭自适应 Thinking");
    expect(preview).toHaveTextContent("当前敏感值（内容已隐藏）");
    expect(preview).toHaveTextContent("本机凭据（内容已隐藏）");
    expect(preview).not.toHaveTextContent("must-not-render-old-secret");
    expect(preview).not.toHaveTextContent("must-not-render-new-secret");
    expect(preview).toHaveTextContent("已加密备份");
    expect(applyAgentPlan).not.toHaveBeenCalled();

    vi.mocked(revealAgentPlanSensitiveValues).mockResolvedValue([{
      target_config_path: "/Users/x/.claude/settings.json",
      path: { segments: ["env", "ANTHROPIC_AUTH_TOKEN"] },
      before_preview: '"sk-ant-old-secret"',
      after_preview: '"ts-local-credential"',
    }]);
    await user.click(within(preview).getByRole("button", { name: "显示完整值" }));
    const revealWarning = await screen.findByRole("alertdialog", { name: "显示敏感配置完整值？" });
    expect(revealWarning).toHaveTextContent("真实的上游 API Key");
    expect(revealAgentPlanSensitiveValues).not.toHaveBeenCalled();
    await user.click(within(revealWarning).getByRole("button", { name: "显示完整值" }));
    await waitFor(() => expect(preview).toHaveTextContent('"sk-ant-old-secret"'));
    expect(preview).toHaveTextContent('"ts-local-credential"');
    expect(revealAgentPlanSensitiveValues).toHaveBeenCalledWith("operation-1", "confirmation-1");
    await user.click(within(preview).getByRole("button", { name: "隐藏完整值" }));
    expect(preview).not.toHaveTextContent("sk-ant-old-secret");
    expect(preview).not.toHaveTextContent("ts-local-credential");

    await user.click(within(preview).getByRole("button", { name: "确认接入" }));
    await waitFor(() => expect(onRefreshAgents).toHaveBeenCalledOnce());

    expect(vi.mocked(ensureServeRunning).mock.invocationCallOrder[0])
      .toBeLessThan(vi.mocked(planAgentConnection).mock.invocationCallOrder[0]);
    expect(vi.mocked(planAgentConnection).mock.invocationCallOrder[0])
      .toBeLessThan(vi.mocked(applyAgentPlan).mock.invocationCallOrder[0]);
    expect(vi.mocked(applyAgentPlan).mock.invocationCallOrder[0])
      .toBeLessThan(onRefreshAgents.mock.invocationCallOrder[0]);
  });

  it("接入后可以只读回看当时的修改并在关闭时丢弃计划", async () => {
    const user = userEvent.setup();
    const found = installation("/opt/homebrew/bin/claude", "2.1.211");
    found.managed = true;
    found.connected = true;
    found.compatibility.status = "CONNECTED";
    const agent: AgentView = {
      metadata: {
        agent_id: "claude-code",
        legacy_kind: "cc",
        display_name: "Claude Code",
        icon_key: "claude",
        admission: "supported",
      },
      installations: [found],
      status: "CONNECTED",
      catalog_sequence: 1,
      catalog_expires_at_ms: null,
      catalog_source: "builtin",
      catalog_warning: null,
    };
    vi.mocked(planAgentDisconnect).mockResolvedValueOnce({
      operation_id: "review-operation",
      confirmation_token: "review-confirmation",
      target_config_path: "/Users/x/.claude/settings.json",
      changes: [],
      projection: {
        schema_version: 1,
        files: [{
          target_config_path: "/Users/x/.claude/settings.json",
          forward_changes: [{
            operation: "replace",
            path: { segments: ["env", "ANTHROPIC_BASE_URL"] },
            sensitive: false,
            summary: "<恢复接管前受管值>",
            before_preview: '"http://127.0.0.1:8787/agents/claude-code"',
            after_preview: '"https://api.anthropic.com"',
          }],
          reverse_changes: [{
            operation: "replace",
            path: { segments: ["env", "ANTHROPIC_BASE_URL"] },
            sensitive: false,
            summary: "<恢复接管前受管值>",
            before_preview: '"https://api.anthropic.com"',
            after_preview: '"http://127.0.0.1:8787/agents/claude-code"',
          }],
        }],
      },
      human_diff: "restore endpoint",
    } as never);

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
        onRefreshAgents={vi.fn()}
        onSaveQuota={vi.fn()}
        onSaveQuotaPlan={vi.fn()}
        onViewQuotaUsage={vi.fn()}
      />,
    );

    await user.click(screen.getByRole("button", { name: "查看接入改动" }));
    const review = await screen.findByRole("dialog", { name: "接入改动" });
    expect(review).toHaveTextContent('"https://api.anthropic.com"');
    expect(review).toHaveTextContent('"http://127.0.0.1:8787/agents/claude-code"');
    expect(review).toHaveTextContent("这是只读记录");
    expect(within(review).queryByRole("button", { name: "确认接入" })).not.toBeInTheDocument();
    expect(applyAgentPlan).not.toHaveBeenCalled();

    const footer = review.querySelector('[data-slot="dialog-footer"]');
    expect(footer).not.toBeNull();
    await user.click(
      within(footer as HTMLElement).getByRole("button", { name: "关闭" }),
    );
    expect(discardAgentPlan).toHaveBeenCalledWith("review-operation", "review-confirmation");
    expect(applyAgentPlan).not.toHaveBeenCalled();
  });

  it("OpenCode 路由模型缺少输出上限时显示持久修复原因并禁止接入", () => {
    const found = installation("/opt/homebrew/bin/opencode", "1.18.2");
    found.discovery.agent_id = "opencode";
    found.discovery.is_path_default = true;
    found.discovery.conflict_group = null;
    found.discovery.diagnostics = [];
    found.compatibility = {
      ...found.compatibility,
      agent_id: "opencode",
      status: "DETECTED_VERIFIED",
      reason_code: "DEFAULT_ADMISSION",
      connector_id: "opencode-v1",
    };
    Object.assign(found, {
      connection_issue: {
        code: "model_contract_missing_max_output_tokens",
        message: "display copy",
        target: "kimi/kimi-k3",
      },
    });
    const agent: AgentView = {
      metadata: {
        agent_id: "opencode",
        legacy_kind: "opencode",
        display_name: "OpenCode",
        icon_key: "opencode",
        admission: "supported",
      },
      installations: [found],
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
          routing_mode: "direct",
          direct_target: { upstream: "kimi", model: "kimi-k3" },
        }}
        providers={[]}
        profiles={[]}
        quotaAccounts={[]}
        serveRunning
        applying={false}
        onStateChange={vi.fn()}
        onRefreshAgents={vi.fn()}
        onSaveQuota={vi.fn()}
        onSaveQuotaPlan={vi.fn()}
        onViewQuotaUsage={vi.fn()}
      />,
    );

    expect(screen.getByText("路由待完善")).toBeInTheDocument();
    expect(screen.getByText(/kimi\/kimi-k3.*最大输出 Token/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "预览并接入" })).toBeDisabled();
  });

  it("版本不兼容优先于 OpenCode 路由契约问题", () => {
    const found = installation("/opt/homebrew/bin/opencode", "0.0.1");
    found.discovery.agent_id = "opencode";
    found.discovery.is_path_default = true;
    found.discovery.conflict_group = null;
    found.compatibility = {
      ...found.compatibility,
      agent_id: "opencode",
      status: "DETECTED_BLOCKED",
      reason_code: "VERSION_BLOCKED",
      message: "version is blocked",
      connector_id: "opencode-v1",
    };
    found.connection_issue = {
      code: "model_contract_missing_max_output_tokens",
      message: "display copy",
      target: "kimi/kimi-k3",
    };
    const agent: AgentView = {
      metadata: { agent_id: "opencode", legacy_kind: "opencode", display_name: "OpenCode", icon_key: "opencode", admission: "supported" },
      installations: [found], status: "DETECTED_BLOCKED", catalog_sequence: 1,
      catalog_expires_at_ms: null, catalog_source: "builtin", catalog_warning: null,
    };

    render(<AgentRoutePage metadata={agent.metadata} agent={agent} route={{ mode: "inherit", tiers: { high: { upstream: null, model: null }, mid: { upstream: null, model: null }, low: { upstream: null, model: null } }, config_error: null, profile: null, routing_mode: "direct" }} providers={[]} profiles={[]} quotaAccounts={[]} serveRunning applying={false} onStateChange={vi.fn()} onRefreshAgents={vi.fn()} onSaveQuota={vi.fn()} onSaveQuotaPlan={vi.fn()} onViewQuotaUsage={vi.fn()} />);

    expect(screen.getByText("暂不可接入")).toBeInTheDocument();
    expect(screen.queryByText("路由待完善")).not.toBeInTheDocument();
  });

  it("取消接入预览不会写入或刷新接管状态", async () => {
    const user = userEvent.setup();
    const found = installation("/opt/homebrew/bin/claude", "2.1.211");
    found.discovery.is_path_default = true;
    found.discovery.conflict_group = null;
    found.discovery.diagnostics = [];
    found.compatibility = {
      ...found.compatibility,
      status: "DETECTED_VERIFIED",
      reason_code: "DEFAULT_ADMISSION",
      connector_id: "claude-code-v1",
    };
    const agent: AgentView = {
      metadata: {
        agent_id: "claude-code",
        legacy_kind: "cc",
        display_name: "Claude Code",
        icon_key: "claude",
        admission: "supported",
      },
      installations: [found],
      status: "DETECTED_VERIFIED",
      catalog_sequence: 1,
      catalog_expires_at_ms: null,
      catalog_source: "builtin",
      catalog_warning: null,
    };
    vi.mocked(planAgentConnection).mockResolvedValue({
      operation_id: "operation-1",
      confirmation_token: "confirmation-1",
      changes: [],
      human_diff: "endpoint changed",
    } as never);
    const onRefreshAgents = vi.fn().mockResolvedValue(undefined);

    render(
      <ErrorToastProvider>
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
          serveRunning={false}
          applying={false}
          onStateChange={vi.fn()}
          onRefreshAgents={onRefreshAgents}
          onSaveQuota={vi.fn()}
          onSaveQuotaPlan={vi.fn()}
          onViewQuotaUsage={vi.fn()}
        />
      </ErrorToastProvider>,
    );

    await user.click(screen.getByRole("button", { name: "预览并接入" }));
    const preview = await screen.findByRole("dialog", { name: "确认接入改动" });
    await user.click(within(preview).getByRole("button", { name: "取消" }));

    expect(screen.queryByRole("dialog", { name: "确认接入改动" })).toBeNull();
    expect(applyAgentPlan).not.toHaveBeenCalled();
    expect(onRefreshAgents).not.toHaveBeenCalled();
  });

  it("connection mode uses one action that becomes Restore after Cursor connects", async () => {
    const user = userEvent.setup();
    const found = installation("/Applications/Cursor.app/Contents/MacOS/Cursor", "1.0.0");
    found.discovery.agent_id = "cursor";
    found.discovery.is_path_default = true;
    found.discovery.conflict_group = null;
    found.discovery.diagnostics = [];
    found.compatibility = {
      ...found.compatibility,
      agent_id: "cursor",
      status: "DETECTED_UNKNOWN",
      reason_code: "CONNECTOR_BINDING_NOT_UNIQUE",
      message: "无法唯一确定该 Agent 的配置 Connector",
      connector_id: null,
      allowed_actions: ["view_details", "rescan", "export_diagnostics"],
    };
    const agent: AgentView = {
      metadata: {
        agent_id: "cursor",
        legacy_kind: null,
        display_name: "Cursor",
        icon_key: "cursor",
        admission: "supported",
      },
      installations: [found],
      status: "DETECTED_VERIFIED",
      catalog_sequence: 1,
      catalog_expires_at_ms: null,
      catalog_source: "builtin",
      catalog_warning: null,
    };

    render(
      <ErrorToastProvider>
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
          serveRunning={false}
          applying={false}
          onStateChange={vi.fn()}
          onRefreshAgents={vi.fn().mockResolvedValue(undefined)}
          onSaveQuota={vi.fn()}
          onSaveQuotaPlan={vi.fn()}
          onViewQuotaUsage={vi.fn()}
          pageMode="connection"
        />
      </ErrorToastProvider>,
    );

    expect(screen.getByText("可接入")).toBeInTheDocument();
    expect(screen.queryByText("暂不可接入")).toBeNull();
    expect(screen.queryByRole("button", { name: "恢复官方配置并断开" })).toBeNull();
    const connectButton = await screen.findByRole("button", { name: "一键接入并启动" });
    await user.click(connectButton);

    const toastViewport = screen.getByTestId("error-toast-viewport");
    expect(await within(toastViewport).findByRole("status")).toHaveTextContent("Cursor 已配置");
    const restoreButton = await screen.findByRole("button", { name: "恢复官方配置并断开" });
    expect(screen.queryByRole("button", { name: "重新接入" })).toBeNull();
    await user.click(restoreButton);
    const restoreDialog = await screen.findByRole("alertdialog", { name: "恢复 Cursor 配置？" });
    await user.click(within(restoreDialog).getByRole("button", { name: "恢复并断开" }));

    expect(restoreCursorProvider).toHaveBeenCalledOnce();
    expect(await screen.findByRole("button", { name: "一键接入并启动" })).toBeEnabled();
    expect(within(toastViewport).getAllByRole("status").some((toast) =>
      toast.textContent?.includes("已恢复 Cursor 官方配置并断开"),
    )).toBe(true);
    expect(document.querySelector(".agent-route-page .banner")).toBeNull();
  });

  it("Cursor 隧道失效后可以直接重新接入，也可以恢复官方配置", async () => {
    vi.mocked(getCursorProviderStatus).mockResolvedValueOnce({
      state: "repair_required",
      message: "上次 Cursor 隧道已失效，请重新接入或恢复官方配置",
    });
    const user = userEvent.setup();
    const found = installation("/Applications/Cursor.app/Contents/MacOS/Cursor", "1.0.0");
    found.discovery.agent_id = "cursor";
    found.discovery.is_path_default = true;
    found.discovery.conflict_group = null;
    found.discovery.diagnostics = [];
    const agent: AgentView = {
      metadata: {
        agent_id: "cursor",
        legacy_kind: null,
        display_name: "Cursor",
        icon_key: "cursor",
        admission: "supported",
      },
      installations: [found],
      status: "DETECTED_VERIFIED",
      catalog_sequence: 1,
      catalog_expires_at_ms: null,
      catalog_source: "builtin",
      catalog_warning: null,
    };

    render(
      <ErrorToastProvider>
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
          onRefreshAgents={vi.fn().mockResolvedValue(undefined)}
          onSaveQuota={vi.fn()}
          onSaveQuotaPlan={vi.fn()}
          onViewQuotaUsage={vi.fn()}
        />
      </ErrorToastProvider>,
    );

    expect(await screen.findByText("需修复")).toBeInTheDocument();
    const reconnect = screen.getByRole("button", { name: "重新接入并启动" });
    expect(screen.getByRole("button", { name: "恢复官方配置并断开" })).toBeEnabled();
    await user.click(reconnect);

    await waitFor(() => expect(configureCursorProvider).toHaveBeenCalledOnce());
    expect(restoreCursorProvider).not.toHaveBeenCalled();
  });

  it("Cursor 运行时显示安全退出提示并恢复接入按钮", async () => {
    window.localStorage.setItem("token-station-language", "zh-CN");
    vi.mocked(configureCursorProvider).mockRejectedValueOnce({
      code: "cursor_running",
      message: "Cursor 正在运行。请手动退出 Cursor 后再点一键接入。",
      target: null,
      stage: null,
      recovery: null,
      recovery_reason_code: null,
    });
    const user = userEvent.setup();
    const onRefreshAgents = vi.fn().mockResolvedValue(undefined);
    const found = installation("/Applications/Cursor.app/Contents/MacOS/Cursor", "1.0.0");
    found.discovery.agent_id = "cursor";
    found.discovery.is_path_default = true;
    found.discovery.conflict_group = null;
    found.discovery.diagnostics = [];
    found.compatibility = {
      ...found.compatibility,
      agent_id: "cursor",
      status: "DETECTED_VERIFIED",
      reason_code: "DEFAULT_ADMISSION",
      connector_id: "cursor-v1",
    };
    const agent: AgentView = {
      metadata: {
        agent_id: "cursor",
        legacy_kind: null,
        display_name: "Cursor",
        icon_key: "cursor",
        admission: "supported",
      },
      installations: [found],
      status: "DETECTED_VERIFIED",
      catalog_sequence: 1,
      catalog_expires_at_ms: null,
      catalog_source: "builtin",
      catalog_warning: null,
    };

    render(
      <ErrorToastProvider>
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
          serveRunning={false}
          applying={false}
          onStateChange={vi.fn()}
          onRefreshAgents={onRefreshAgents}
          onSaveQuota={vi.fn()}
          onSaveQuotaPlan={vi.fn()}
          onViewQuotaUsage={vi.fn()}
        />
      </ErrorToastProvider>,
    );

    await user.click(screen.getByRole("button", { name: "一键接入并启动" }));

    const toastViewport = screen.getByTestId("error-toast-viewport");
    const alert = await within(toastViewport).findByRole("alert");
    expect(alert).toHaveTextContent("Cursor 仍在运行");
    expect(alert).toHaveTextContent("请彻底退出 Cursor 后再点一次一键接入");
    expect(alert).not.toHaveTextContent("操作未能完成");
    expect(within(toastViewport).queryByRole("status")).toBeNull();
    await waitFor(() => expect(onRefreshAgents).toHaveBeenCalledOnce());
    expect(screen.getByRole("button", { name: "一键接入并启动" })).toBeEnabled();
  });

  it("没有策略组时用错误 Toast 提示且页面不渲染错误横条", async () => {
    const user = userEvent.setup();

    render(
      <ErrorToastProvider>
        <AgentRoutePage
          metadata={{
            agent_id: "claude-code",
            legacy_kind: "cc",
            display_name: "Claude Code",
            icon_key: "claude",
            admission: "supported",
          }}
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
          onRefreshAgents={vi.fn()}
          onSaveQuota={vi.fn()}
          onSaveQuotaPlan={vi.fn()}
          onViewQuotaUsage={vi.fn()}
        />
      </ErrorToastProvider>,
    );

    await user.click(screen.getByRole("radio", { name: "挂载策略组" }));

    const toastViewport = screen.getByTestId("error-toast-viewport");
    expect(within(toastViewport).getByRole("alert")).toHaveTextContent("还没有可挂载的策略组");
    expect(mountAgentProfile).not.toHaveBeenCalled();
    expect(document.querySelector(".agent-route-page .banner")).toBeNull();
  });

  it("路由应用和恢复主页成功后用 Toast 提示，不生成页面横条", async () => {
    const user = userEvent.setup();
    const commonProps = {
      metadata: {
        agent_id: "claude-code",
        legacy_kind: "cc" as const,
        display_name: "Claude Code",
        icon_key: "claude",
        admission: "supported" as const,
      },
      providers: [],
      profiles: [],
      quotaAccounts: [],
      serveRunning: true,
      applying: false,
      onStateChange: vi.fn(),
      onRefreshAgents: vi.fn(),
      onSaveQuota: vi.fn(),
      onSaveQuotaPlan: vi.fn(),
      onViewQuotaUsage: vi.fn(),
    };
    const inheritedRoute = {
      mode: "inherit" as const,
      tiers: {
        high: { upstream: null, model: null },
        mid: { upstream: null, model: null },
        low: { upstream: null, model: null },
      },
      config_error: null,
      profile: null,
      routing_mode: "tiered" as const,
    };
    const view = render(
      <ErrorToastProvider>
        <AgentRoutePage {...commonProps} route={inheritedRoute} />
      </ErrorToastProvider>,
    );

    expect(screen.getByRole("button", { name: "应用" }).closest(".panel-head"))
      .toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "应用" }));
    let toastViewport = screen.getByTestId("error-toast-viewport");
    expect(await within(toastViewport).findByRole("status")).toHaveTextContent("已将主页路由应用到此 Agent");
    expect(document.querySelector(".agent-route-page .banner")).toBeNull();

    view.unmount();
    render(
      <ErrorToastProvider>
        <AgentRoutePage
          {...commonProps}
          route={{ ...inheritedRoute, mode: "custom" }}
        />
      </ErrorToastProvider>,
    );
    await user.click(screen.getByRole("button", { name: "恢复主页路由" }));
    toastViewport = screen.getByTestId("error-toast-viewport");
    expect(await within(toastViewport).findByRole("status")).toHaveTextContent("已恢复跟随主页");
    expect(document.querySelector(".agent-route-page .banner")).toBeNull();
  });

  it("恢复前检测手动修改，并让用户选择自行处理或强制恢复备份", async () => {
    const user = userEvent.setup();
    const found = installation("/opt/homebrew/bin/claude", "2.1.211");
    found.managed = true;
    found.connected = true;
    found.compatibility.status = "CONNECTED";
    const agent: AgentView = {
      metadata: {
        agent_id: "claude-code",
        legacy_kind: "cc",
        display_name: "Claude Code",
        icon_key: "claude",
        admission: "supported",
      },
      installations: [found],
      status: "CONNECTED",
      catalog_sequence: 1,
      catalog_expires_at_ms: null,
      catalog_source: "builtin",
      catalog_warning: null,
    };
    vi.mocked(getAgentDrift).mockResolvedValueOnce([{
      agent_id: "claude-code",
      installation_path: found.discovery.canonical_path,
      target_config_path: "/Users/x/.claude/settings.json",
      connector_id: "claude-code-v1",
      status: "managed_changes",
      baseline_hash: "a".repeat(64),
      managed_hash: "b".repeat(64),
      current_hash: "c".repeat(64),
      checked_at_ms: 1,
      changes: [{
        path: { segments: ["env", "ANTHROPIC_BASE_URL"] },
        scope: "managed",
        kind: "changed",
        current_matches_managed: false,
      }, {
        path: { segments: ["theme"] },
        scope: "unowned",
        kind: "changed",
        current_matches_managed: null,
      }],
      truncated: false,
      message: "外部修改触及 Token Station 受管字段",
    }]);
    vi.mocked(planAgentDisconnect).mockResolvedValueOnce({
      operation_id: "restore-operation",
      confirmation_token: "restore-confirmation",
      target_config_path: "/Users/x/.claude/settings.json",
      changes: [{
        operation: "replace",
        path: { segments: ["env", "ANTHROPIC_BASE_URL"] },
        sensitive: false,
        summary: "<恢复接管前受管值>",
      }],
      human_diff: "restore endpoint",
    } as never);
    const onRefreshAgents = vi.fn().mockResolvedValue(undefined);

    render(
      <ErrorToastProvider>
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
          onRefreshAgents={onRefreshAgents}
          onSaveQuota={vi.fn()}
          onSaveQuotaPlan={vi.fn()}
          onViewQuotaUsage={vi.fn()}
        />
      </ErrorToastProvider>,
    );

    await user.click(screen.getByRole("button", { name: "恢复官方配置并断开" }));

    const conflict = await screen.findByRole("alertdialog", { name: "配置文件已被修改" });
    expect(conflict).toHaveTextContent("env.ANTHROPIC_BASE_URL");
    expect(conflict).toHaveTextContent("Token Station 管理的字段");
    expect(conflict).toHaveTextContent("theme");
    expect(conflict).toHaveTextContent("其他字段（将保留）");
    expect(planAgentDisconnect).not.toHaveBeenCalled();
    expect(applyAgentPlan).not.toHaveBeenCalled();

    await user.click(within(conflict).getByRole("button", { name: "强制恢复备份" }));

    await waitFor(() => expect(planAgentDisconnect).toHaveBeenCalledWith(
      "claude-code",
      found.discovery.canonical_path,
    ));
    expect(applyAgentPlan).toHaveBeenCalledWith("restore-operation", "restore-confirmation");
    expect(forceForgetAgent).not.toHaveBeenCalled();
    await waitFor(() => expect(onRefreshAgents).toHaveBeenCalledOnce());
  });

  it("恢复官方配置成功后的缓存刷新失败不反转恢复结果", async () => {
    const user = userEvent.setup();
    const found = installation("/opt/homebrew/bin/claude", "2.1.211");
    found.managed = true;
    found.connected = true;
    found.compatibility.status = "CONNECTED";
    const agent: AgentView = {
      metadata: {
        agent_id: "claude-code",
        legacy_kind: "cc",
        display_name: "Claude Code",
        icon_key: "claude",
        admission: "supported",
      },
      installations: [found],
      status: "CONNECTED",
      catalog_sequence: 1,
      catalog_expires_at_ms: null,
      catalog_source: "builtin",
      catalog_warning: null,
    };
    vi.mocked(planAgentDisconnect).mockResolvedValueOnce({
      operation_id: "restore-operation",
      confirmation_token: "restore-confirmation",
      target_config_path: "/Users/x/.claude/settings.json",
      changes: [],
      human_diff: "restore",
    } as never);

    render(
      <ErrorToastProvider>
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
          onRefreshAgents={vi.fn().mockRejectedValue(new Error("refresh failed"))}
          onSaveQuota={vi.fn()}
          onSaveQuotaPlan={vi.fn()}
          onViewQuotaUsage={vi.fn()}
        />
      </ErrorToastProvider>,
    );

    await user.click(screen.getByRole("button", { name: "恢复官方配置并断开" }));
    const restorePreview = await screen.findByRole("dialog", { name: "确认恢复备份" });
    await user.click(within(restorePreview).getByRole("button", { name: "恢复备份并断开" }));

    const viewport = screen.getByTestId("error-toast-viewport");
    expect(await within(viewport).findByRole("status")).toHaveTextContent("已恢复备份并断开");
    expect(within(viewport).getByRole("alert")).toHaveTextContent("操作未能完成");
    expect(document.querySelector(".agent-route-page .banner")).toBeNull();
  });

  it("ensure 成功但 plan 失败时解锁后仍刷新一次缓存态", async () => {
    const user = userEvent.setup();
    const found = installation("/opt/homebrew/bin/claude", "2.1.211");
    found.discovery.is_path_default = true;
    found.discovery.conflict_group = null;
    found.discovery.diagnostics = [];
    found.compatibility = {
      ...found.compatibility,
      status: "DETECTED_VERIFIED",
      reason_code: "DEFAULT_ADMISSION",
      connector_id: "claude-code-v1",
    };
    const agent: AgentView = {
      metadata: {
        agent_id: "claude-code",
        legacy_kind: "cc",
        display_name: "Claude Code",
        icon_key: "claude",
        admission: "supported",
      },
      installations: [found],
      status: "DETECTED_VERIFIED",
      catalog_sequence: 1,
      catalog_expires_at_ms: null,
      catalog_source: "builtin",
      catalog_warning: null,
    };
    vi.mocked(planAgentConnection).mockRejectedValue(new Error("plan failed"));
    const onRefreshAgents = vi.fn().mockResolvedValue(undefined);
    const onConnectInFlightChange = vi.fn();

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
        serveRunning={false}
        applying={false}
        onStateChange={vi.fn()}
        onRefreshAgents={onRefreshAgents}
        onConnectInFlightChange={onConnectInFlightChange}
        onSaveQuota={vi.fn()}
        onSaveQuotaPlan={vi.fn()}
        onViewQuotaUsage={vi.fn()}
      />,
    );

    await user.click(screen.getByRole("button", { name: "预览并接入" }));
    await waitFor(() => expect(onRefreshAgents).toHaveBeenCalledOnce());

    expect(applyAgentPlan).not.toHaveBeenCalled();
    expect(onConnectInFlightChange).toHaveBeenNthCalledWith(1, true);
    expect(onConnectInFlightChange).toHaveBeenNthCalledWith(2, false);
    expect(onConnectInFlightChange.mock.invocationCallOrder[1])
      .toBeLessThan(onRefreshAgents.mock.invocationCallOrder[0]);
    expect(screen.getByRole("button", { name: "预览并接入" })).toBeEnabled();
  });

  it("does not expose a raw compatibility message in English mode", () => {
    window.localStorage.setItem("token-station-language", "en");
    const blocked = installation("/opt/homebrew/bin/claude", "1.0.0");
    blocked.discovery.is_path_default = true;
    blocked.discovery.conflict_group = null;
    blocked.compatibility = {
      ...blocked.compatibility,
      status: "DETECTED_BLOCKED",
      reason_code: "BLOCKED_VERSION_MATCH",
      message: "当前版本在阻断列表中：/Users/example/private",
      allowed_actions: ["view_details"],
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
      installations: [blocked],
      status: "DETECTED_BLOCKED",
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
        onRefreshAgents={vi.fn()}
        onSaveQuota={vi.fn()}
        onSaveQuotaPlan={vi.fn()}
        onViewQuotaUsage={vi.fn()}
      />,
    );

    expect(screen.getByText(
      "The operation could not be completed. Try again. If it still fails, update Token Station or contact support.",
    )).toBeInTheDocument();
    expect(screen.queryByText(/当前版本在阻断列表/)).not.toBeInTheDocument();
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
        onRefreshAgents={vi.fn()}
        onSaveQuota={vi.fn()}
        onSaveQuotaPlan={vi.fn()}
        onViewQuotaUsage={vi.fn()}
      />,
    );

    const connect = screen.getByRole("button", { name: "预览并接入" });
    expect(connect).toBeDisabled();
    await user.click(screen.getByRole("button", { name: /选择版本/ }));
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
    vi.mocked(planAgentDisconnect).mockResolvedValueOnce({
      operation_id: "repair-restore",
      confirmation_token: "repair-confirmation",
      target_config_path: "/Users/x/.config/opencode/opencode.json",
      changes: [],
      human_diff: "restore",
    } as never);

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
        onRefreshAgents={vi.fn()}
        onSaveQuota={vi.fn()}
        onSaveQuotaPlan={vi.fn()}
        onViewQuotaUsage={vi.fn()}
      />,
    );

    expect(screen.getByText("需修复")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "恢复官方配置并断开" }));

    await waitFor(() => expect(getAgentDrift).toHaveBeenCalledWith(
      "opencode",
      "/opt/homebrew/bin/opencode",
    ));
    expect(await screen.findByRole("dialog", { name: "确认恢复备份" })).toBeInTheDocument();
    expect(forceForgetAgent).not.toHaveBeenCalled();
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
        onRefreshAgents={vi.fn()}
        onSaveQuota={vi.fn()}
        onSaveQuotaPlan={vi.fn()}
        onViewQuotaUsage={vi.fn()}
      />,
    );

    expect(await screen.findByText("适配器未就绪")).toBeInTheDocument();
    expect(screen.getByText(/Agent 配置未被修改/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "预览并接入" })).toBeDisabled();
    expect(planAgentConnection).not.toHaveBeenCalled();
  });

  it("keeps Cursor visible but disables writes when ownership state is unavailable", async () => {
    const cursor = installation(
      "/Applications/Cursor.app/Contents/MacOS/Cursor",
      "unknown",
    );
    cursor.adapter_ready = null;
    cursor.discovery.agent_id = "cursor";
    cursor.discovery.is_path_default = true;
    cursor.discovery.conflict_group = null;
    cursor.discovery.version_raw = null;
    cursor.discovery.version_normalized = null;
    cursor.discovery.diagnostics = [{
      reason_code: "READ_ONLY_PREFLIGHT_FAILED",
      message: "ownership unavailable",
    }];
    cursor.compatibility = {
      agent_id: "cursor",
      installation_path: cursor.discovery.canonical_path,
      status: "DETECTED_UNKNOWN",
      reason_code: "READ_ONLY_PREFLIGHT_FAILED",
      message: "只读配置预检未通过，当前安装不能接入",
      matched_catalog_version: "builtin",
      connector_id: null,
      allowed_actions: ["view_details", "rescan", "export_diagnostics"],
    };
    const agent: AgentView = {
      metadata: {
        agent_id: "cursor",
        legacy_kind: null,
        display_name: "Cursor",
        icon_key: "cursor",
        admission: "discovery_only",
        ui_order: 45,
        nav_mark: "C",
      },
      installations: [cursor],
      status: "DETECTED_UNKNOWN",
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
        onRefreshAgents={vi.fn()}
        onSaveQuota={vi.fn()}
        onSaveQuotaPlan={vi.fn()}
        onViewQuotaUsage={vi.fn()}
      />,
    );

    expect(screen.getByText("接管状态不可用")).toBeInTheDocument();
    expect(screen.getByText(/Agent 仍可只读显示/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "一键接入并启动" })).toBeDisabled();
  });
});

describe("compactDiscoveryPath", () => {
  it("does not alias another macOS user's directory as the current home", () => {
    expect(compactDiscoveryPath(
      "/Users/admin/.local/lib/node_modules/@anthropic-ai/claude-code/bin/claude",
    )).toBe("/Users/admin/…/@anthropic-ai/claude-code/bin/claude");
  });

  it("preserves the Windows drive and user boundary when compacting", () => {
    expect(compactDiscoveryPath(
      "C:\\Users\\x\\AppData\\Roaming\\npm\\node_modules\\@anthropic-ai\\claude-code\\bin\\claude.exe",
    )).toBe("C:\\Users\\x\\…\\@anthropic-ai\\claude-code\\bin\\claude.exe");
  });
});

describe("AgentRoutePage split page modes", () => {
  const route = {
    mode: "inherit" as const,
    inherits_global: true,
    tiers: {
      high: { upstream: null, model: null },
      mid: { upstream: null, model: null },
      low: { upstream: null, model: null },
    },
    config_error: null,
    profile: null,
    routing_mode: "direct" as const,
    direct_target: { upstream: "deepseek", model: "deepseek-v4-flash" },
  };
  const metadata = {
    agent_id: "claude-code",
    legacy_kind: "cc",
    display_name: "Claude Code",
    icon_key: "claude",
    admission: "supported" as const,
    connector_capabilities: [{
      connector_id: "claude-code-v1",
      adapter_id: "anthropic",
      base_url_shape: "origin" as const,
      platforms: ["macos" as const],
      config_format: "json",
      config_path_template: "~/.claude/settings.json",
      owned_fields: ["ANTHROPIC_BASE_URL", "ANTHROPIC_AUTH_TOKEN"],
      requires_virtual_key: true,
      restart_required: false,
    }],
  };
  const found = installation("/opt/homebrew/bin/claude", "2.1.211");
  const agent: AgentView = {
    metadata,
    installations: [found],
    status: "DETECTED_VERIFIED",
    catalog_sequence: 1,
    catalog_expires_at_ms: null,
    catalog_source: "builtin",
    catalog_warning: null,
  };
  const props = {
    metadata,
    agent,
    route,
    providers: [],
    profiles: [],
    quotaAccounts: [],
    serveRunning: true,
    applying: false,
    selectedInstallationPath: found.discovery.canonical_path,
    onStateChange: vi.fn(),
    onRefreshAgents: vi.fn(),
    onSaveQuota: vi.fn(),
    onSaveQuotaPlan: vi.fn(),
    onViewQuotaUsage: vi.fn(),
  };

  it("shows discovery and connection controls without routing controls", () => {
    render(<ErrorToastProvider><AgentRoutePage {...props} pageMode="connection" embedded /></ErrorToastProvider>);

    expect(screen.getByText("/opt/homebrew/bin/claude")).toBeInTheDocument();
    expect(screen.getByText("2.1.211")).toBeInTheDocument();
    expect(screen.getByText("/Users/x/.claude/settings.json")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "预览并接入" })).toBeInTheDocument();
    const routePreview = screen.getByText("接入后的路由预览").closest(".agent-default-route-state");
    expect(routePreview).not.toBeNull();
    expect(within(routePreview as HTMLElement).queryByText("✓")).not.toBeInTheDocument();
    expect(screen.queryByText("选择请求如何分配")).toBeNull();
  });

  it("语义缩略长发现路径，并在提示与复制中保留完整绝对路径", async () => {
    const user = userEvent.setup();
    const fullPath = "/Users/liuwenhao/.local/lib/node_modules/@anthropic-ai/claude-code/bin/claude";
    const longInstallation = installation(fullPath, "2.1.211");
    const longAgent = { ...agent, installations: [longInstallation] };
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText },
    });

    render(
      <ErrorToastProvider>
        <AgentRoutePage
          {...props}
          agent={longAgent}
          selectedInstallationPath={fullPath}
          pageMode="connection"
          embedded
        />
      </ErrorToastProvider>,
    );

    const compactPath = screen.getByText("/Users/liuwenhao/…/@anthropic-ai/claude-code/bin/claude");
    await user.hover(compactPath);
    expect(await screen.findByRole("tooltip")).toHaveTextContent(fullPath);
    await user.click(screen.getByRole("button", { name: "复制发现路径" }));
    expect(writeText).toHaveBeenCalledWith(fullPath);
    expect(screen.getByRole("button", { name: "发现路径已复制" })).toBeInTheDocument();
  });

  it("does not carry copied feedback to another installation", async () => {
    const user = userEvent.setup();
    const firstPath = "/Users/x/.local/share/claude/versions/one/bin/claude";
    const secondPath = "/Users/x/.local/share/claude/versions/two/bin/claude";
    const multiAgent = {
      ...agent,
      installations: [installation(firstPath, "1.0.0"), installation(secondPath, "2.0.0")],
    };
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText },
    });
    const view = (selectedInstallationPath: string) => (
      <ErrorToastProvider>
        <AgentRoutePage
          {...props}
          agent={multiAgent}
          selectedInstallationPath={selectedInstallationPath}
          pageMode="connection"
          embedded
        />
      </ErrorToastProvider>
    );
    const { rerender } = render(view(firstPath));

    await user.click(screen.getByRole("button", { name: "复制发现路径" }));
    expect(writeText).toHaveBeenCalledWith(firstPath);
    expect(screen.getByRole("button", { name: "发现路径已复制" })).toBeInTheDocument();

    rerender(view(secondPath));
    expect(screen.getByRole("button", { name: "复制发现路径" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "发现路径已复制" })).toBeNull();
  });

  it("restarts copied feedback after repeating the same copy", async () => {
    vi.useFakeTimers();
    const fullPath = "/Users/x/.local/share/claude/versions/one/bin/claude";
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText },
    });
    render(
      <ErrorToastProvider>
        <AgentRoutePage
          {...props}
          agent={{ ...agent, installations: [installation(fullPath, "1.0.0")] }}
          selectedInstallationPath={fullPath}
          pageMode="connection"
          embedded
        />
      </ErrorToastProvider>,
    );

    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "复制发现路径" }));
      await Promise.resolve();
    });
    act(() => vi.advanceTimersByTime(1_000));
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "发现路径已复制" }));
      await Promise.resolve();
    });
    act(() => vi.advanceTimersByTime(700));

    expect(writeText).toHaveBeenCalledTimes(2);
    expect(screen.getByRole("button", { name: "发现路径已复制" })).toBeInTheDocument();

    act(() => vi.advanceTimersByTime(900));
    expect(screen.getByRole("button", { name: "复制发现路径" })).toBeInTheDocument();
  });

  it("复制发现路径失败时使用现有错误提示", async () => {
    const user = userEvent.setup();
    const fullPath = "/Users/liuwenhao/.local/lib/node_modules/@anthropic-ai/claude-code/bin/claude";
    const longInstallation = installation(fullPath, "2.1.211");
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText: vi.fn().mockRejectedValue(new Error("clipboard denied")) },
    });

    render(
      <ErrorToastProvider>
        <AgentRoutePage
          {...props}
          agent={{ ...agent, installations: [longInstallation] }}
          selectedInstallationPath={fullPath}
          pageMode="connection"
          embedded
        />
      </ErrorToastProvider>,
    );

    await user.click(screen.getByRole("button", { name: "复制发现路径" }));
    expect(await within(screen.getByTestId("error-toast-viewport")).findByRole("alert"))
      .toHaveTextContent("无法复制发现路径，请检查系统剪贴板权限后重试。");
  });

  it("hides routing strategies for a globally inherited Agent until requested", async () => {
    const user = userEvent.setup();
    render(<ErrorToastProvider><AgentRoutePage {...props} pageMode="routing" embedded /></ErrorToastProvider>);

    expect(screen.getByText("跟随全局路由")).toBeInTheDocument();
    expect(screen.queryByRole("tablist", { name: "Agent 路由策略" })).toBeNull();
    expect(screen.queryByText("选择请求如何分配")).toBeNull();
    expect(screen.queryByRole("button", { name: "预览并接入" })).toBeNull();
    expect(screen.queryByText("发现路径")).toBeNull();

    await user.click(screen.getByRole("button", { name: "设置独立路由" }));
    expect(screen.getByRole("tablist", { name: "Agent 路由策略" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "收起设置" })).toBeInTheDocument();
  });

  it("opens an existing independent route directly with a global restore action", () => {
    render(
      <ErrorToastProvider>
        <AgentRoutePage
          {...props}
          route={{ ...route, inherits_global: false }}
          pageMode="routing"
          embedded
        />
      </ErrorToastProvider>,
    );

    expect(screen.getByRole("tablist", { name: "Agent 路由策略" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "恢复跟随全局" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "设置独立路由" })).toBeNull();
  });
});
