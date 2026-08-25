import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { AgentUiMetadataView, AgentView, StateView } from "../api";
import { LANGUAGE_STORAGE_KEY, LanguageProvider } from "../components/LanguageProvider";
import OverviewPage from "./OverviewPage";

vi.mock("../api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api")>();
  return {
    ...actual,
    getStats: vi.fn().mockResolvedValue({
      total: {
        requests: 0,
        errors: 0,
        p50_latency_ms: 0,
        p95_latency_ms: 0,
        input_tokens: 0,
        output_tokens: 0,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
        reasoning_tokens: 0,
        cost_micros: null,
        priced_requests: 0,
        unpriced_requests: 0,
      },
      groups: [],
      by: null,
      empty: true,
    }),
  };
});

const registry: AgentUiMetadataView[] = Array.from({ length: 6 }, (_, index) => ({
  agent_id: index === 0 ? "claude-code" : `agent-${index}`,
  legacy_kind: null,
  display_name: index === 0 ? "Claude Code" : `Agent ${index}`,
  icon_key: "test",
  admission: "supported",
  nav_mark: `A${index}`,
}));

const agents: AgentView[] = registry.map((metadata, index) => ({
  metadata,
  installations: [],
  status: index < 2 ? "CONNECTED" : "DETECTED_VERIFIED",
  catalog_sequence: index,
  catalog_expires_at_ms: null,
  catalog_source: "builtin",
  catalog_warning: null,
}));

const routeOverride = {
  mode: "custom" as const,
  tiers: {
    high: { upstream: "deepseek-main", model: "deepseek-v4" },
    mid: { upstream: "deepseek-main", model: "deepseek-v4-flash" },
    low: { upstream: "deepseek-main", model: "deepseek-v4-lite" },
  },
  config_error: null,
  profile: null,
  routing_mode: "tiered" as const,
  direct_target: { upstream: "deepseek-main", model: "deepseek-v4-flash" },
};

const state = {
  providers: [
    {
      name: "openai-main",
      brand_id: "openai",
      provider: "openai-compatible",
      base_url: "https://api.openai.com/v1",
      models: ["gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"],
      has_auth: true,
    },
    {
      name: "deepseek-main",
      brand_id: "deepseek",
      provider: "openai-compatible",
      base_url: "https://api.deepseek.com/v1",
      models: ["deepseek-v4", "deepseek-v4-flash", "deepseek-v4-lite"],
      has_auth: true,
    },
  ],
  tiers: {
    high: { upstream: null, model: null },
    mid: { upstream: null, model: null },
    low: { upstream: null, model: null },
  },
  keywords: { high: [], mid: [], low: [] },
  agent_routes: { "agent-1": routeOverride },
  profiles: [],
  local_only: false,
  allow_cloud_fallback: false,
  routing_mode: "direct",
  direct_target: { upstream: "openai-main", model: "gpt-5.6-sol" },
  quota_accounts: [],
  serve: {
    phase: "stopped",
    app_runtime: "stopped",
    listener_reachable: false,
    agent_connected: false,
    running_revision: null,
    instance_id: null,
    listen: "127.0.0.1:8787",
    virtual_key: null,
    error: null,
  },
  draft_revision: 1,
  saved_revision: 1,
  config_dirty: false,
  config_error: null,
  settings: {
    listen: "127.0.0.1:8787",
    auth: true,
    metrics: true,
    data_dir: "/tmp/token-station",
    plugins_dir: "/tmp/token-station/plugins",
    agent: "test",
    version: "test",
    egress_mode: "direct",
    egress_proxy_url: "",
    egress_no_proxy: [],
    egress_auth_username: "",
    egress_auth_slot: "",
  },
} satisfies StateView;

beforeEach(() => {
  window.localStorage.setItem(LANGUAGE_STORAGE_KEY, "zh-CN");
});

describe("OverviewPage summaries", () => {
  it("renders the Overview title and content in Japanese", () => {
    window.localStorage.setItem(LANGUAGE_STORAGE_KEY, "ja");

    render(
      <LanguageProvider>
        <OverviewPage state={state} registry={registry} agents={agents} onNavigate={vi.fn()} />
      </LanguageProvider>,
    );

    expect(screen.getByRole("heading", { name: "概要" })).toBeInTheDocument();
    expect(screen.getByText("プロキシのステータス、現在のルーティング、リクエストとコストを一画面で確認できます。"))
      .toBeInTheDocument();
    expect(screen.getByText("プロキシステータス")).toBeInTheDocument();
    expect(screen.getByText("リビジョン")).toBeInTheDocument();
    expect(screen.getByRole("region", { name: "Agentの概要" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Overview" })).toBeNull();
  });

  it("renders the Overview title and content in Traditional Chinese", () => {
    window.localStorage.setItem(LANGUAGE_STORAGE_KEY, "zh-TW");

    render(
      <LanguageProvider>
        <OverviewPage state={state} registry={registry} agents={agents} onNavigate={vi.fn()} />
      </LanguageProvider>,
    );

    expect(screen.getByRole("heading", { name: "概覽" })).toBeInTheDocument();
    expect(screen.getByText("代理執行狀態、當前路由、請求與成本，一屏看清。"))
      .toBeInTheDocument();
    expect(screen.getByText("代理狀態")).toBeInTheDocument();
    expect(screen.getByText("版本")).toBeInTheDocument();
    expect(screen.getByText("已連線 2 個 Agent")).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Overview" })).toBeNull();
  });

  it("moves connected Agents to the front and restores registry order after disconnect", () => {
    const laterAgentConnected = agents.map((agent, index) => ({
      ...agent,
      status: index === 4 ? "CONNECTED" as const : "DETECTED_VERIFIED" as const,
    }));
    const { rerender } = render(
      <LanguageProvider>
        <OverviewPage state={state} registry={registry} agents={laterAgentConnected} onNavigate={vi.fn()} />
      </LanguageProvider>,
    );
    const agentSummary = screen.getByRole("region", { name: "Agent 概览" });
    const rowNames = () => within(agentSummary).getAllByRole("listitem")
      .map((row) => row.querySelector("strong")?.textContent);

    expect(rowNames()).toEqual(["Agent 4", "Claude Code", "Agent 1", "Agent 2", "Agent 3"]);

    rerender(
      <LanguageProvider>
        <OverviewPage
          state={state}
          registry={registry}
          agents={laterAgentConnected.map((agent) => ({ ...agent, status: "DETECTED_VERIFIED" }))}
          onNavigate={vi.fn()}
        />
      </LanguageProvider>,
    );

    expect(rowNames()).toEqual(["Claude Code", "Agent 1", "Agent 2", "Agent 3", "Agent 4"]);
  });

  it("shows fixed Agent, routing, and model summaries capped at five rows", () => {
    render(
      <LanguageProvider>
        <OverviewPage state={state} registry={registry} agents={agents} onNavigate={vi.fn()} />
      </LanguageProvider>,
    );

    const agentSummary = screen.getByRole("region", { name: "Agent 概览" });
    expect(within(agentSummary).getByText("已接入 2 个 Agent")).toBeInTheDocument();
    expect(within(agentSummary).queryByText("2 个已接管")).toBeNull();
    expect(within(agentSummary).getAllByRole("listitem")).toHaveLength(5);

    const routeSummary = screen.getByRole("region", { name: "路由概览" });
    expect(within(routeSummary).queryByTestId("revision-chain")).toBeNull();
    expect(within(routeSummary).getAllByRole("listitem")).toHaveLength(2);
    const globalRoute = within(routeSummary).getByText("Claude Code", { selector: "strong" }).closest("li");
    const customRoute = within(routeSummary).getByText("Agent 1", { selector: "strong" }).closest("li");
    expect(globalRoute).toHaveTextContent("全局 · 简单路由");
    expect(globalRoute).not.toHaveTextContent("gpt-5.6-sol");
    expect(customRoute).toHaveTextContent("独立 · 智能路由");
    expect(customRoute).not.toHaveTextContent("deepseek-v4");

    const modelSummary = screen.getByRole("region", { name: "模型概览" });
    expect(within(modelSummary).getByText("6 个模型")).toBeInTheDocument();
    expect(within(modelSummary).getAllByRole("listitem")).toHaveLength(5);
    const firstModel = within(modelSummary).getAllByRole("listitem")[0];
    expect(firstModel.textContent?.indexOf("gpt-5.6-sol"))
      .toBeLessThan(firstModel.textContent?.indexOf("openai-main") ?? -1);

    expect(screen.getByRole("button", { name: "打开 Agent" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "打开路由" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "打开模型" })).toBeInTheDocument();
  });

  it("shows only the global route snapshot when no Agent is connected", () => {
    render(
      <LanguageProvider>
        <OverviewPage
          state={state}
          registry={registry}
          agents={agents.map((agent) => ({ ...agent, status: "DETECTED_VERIFIED" }))}
          onNavigate={vi.fn()}
        />
      </LanguageProvider>,
    );

    const routeSummary = screen.getByRole("region", { name: "路由概览" });
    expect(within(routeSummary).getByText("全局路由")).toBeInTheDocument();
    expect(within(routeSummary).getByText("简单路由")).toBeInTheDocument();
    expect(within(routeSummary).getByText("gpt-5.6-sol")).toBeInTheDocument();
    expect(within(routeSummary).getByText("openai-main")).toBeInTheDocument();
    expect(within(routeSummary).queryByRole("listitem")).toBeNull();
    expect(within(routeSummary).queryByText("Claude Code", { selector: "strong" })).toBeNull();
  });

  it("opens the model test console from one clear Overview action", async () => {
    const user = userEvent.setup();
    render(
      <LanguageProvider>
        <OverviewPage state={state} registry={registry} agents={agents} onNavigate={vi.fn()} />
      </LanguageProvider>,
    );

    const agentSummary = screen.getByRole("region", { name: "Agent 概览" });
    const agentActions = agentSummary.querySelector<HTMLElement>(".overview-agent-actions");
    expect(agentActions).not.toBeNull();
    const testButton = within(agentActions!).getByRole("button", { name: "验证模型连接" });
    const openAgentsButton = within(agentActions!).getByRole("button", { name: "打开 Agent" });
    expect(testButton.compareDocumentPosition(openAgentsButton) & Node.DOCUMENT_POSITION_FOLLOWING)
      .toBeTruthy();
    expect(document.querySelector(".overview-heading")?.contains(testButton)).toBe(false);

    await user.click(testButton);

    expect(screen.getByRole("dialog", { name: "测试模型" })).toBeInTheDocument();
  });

  it("keeps the model test action visible but disabled without configured models", () => {
    render(
      <LanguageProvider>
        <OverviewPage
          state={{ ...state, providers: [], direct_target: null }}
          registry={registry}
          agents={agents}
          onNavigate={vi.fn()}
        />
      </LanguageProvider>,
    );

    expect(screen.getByRole("button", { name: "验证模型连接" })).toBeDisabled();
  });
});
