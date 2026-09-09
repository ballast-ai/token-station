import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { AgentRouteView } from "../api";
import AgentCapabilityGuide from "./AgentCapabilityGuide";

const route: AgentRouteView = {
  mode: "inherit", inherits_global: true, routing_mode: "direct",
  direct_target: { upstream: "wecoding", model: "glm-5.2" },
  tiers: { high: { upstream: null, model: null }, mid: { upstream: null, model: null }, low: { upstream: null, model: null } },
  config_error: null, profile: null,
};

describe("Agent route and search guidance", () => {
  it("shows the effective direct target even before an Agent connects", () => {
    render(<AgentCapabilityGuide agentId="deepseek-harness" route={route} />);
    expect(screen.getByText("跟随全局")).toBeInTheDocument();
    expect(screen.getByText("wecoding / glm-5.2")).toBeInTheDocument();
    expect(screen.queryByText(/网关正常|搜索可用|搜索已通过/)).not.toBeInTheDocument();
  });

  it("does not mistake inherited tiers for full route inheritance", () => {
    render(<AgentCapabilityGuide agentId="deepseek-harness" route={{
      ...route, inherits_global: false, direct_target: { upstream: "kimi", model: "kimi-k3" },
    }} />);
    expect(screen.getByText("独立路由")).toBeInTheDocument();
    expect(screen.getByText("kimi / kimi-k3")).toBeInTheDocument();
    expect(screen.queryByText("跟随全局")).not.toBeInTheDocument();
  });

  it("does not claim a target when a direct configuration is incomplete", () => {
    render(<AgentCapabilityGuide agentId="kimi-code" route={{ ...route, direct_target: null }} />);
    expect(screen.getByText("尚未选择供应商和模型")).toBeInTheDocument();
  });

  it("makes the scoped OpenCode activation command available in a focusable disclosure", async () => {
    const user = userEvent.setup();
    render(<AgentCapabilityGuide agentId="opencode" route={route} />);
    const details = screen.getByText("网络搜索配置").closest("details")!;
    expect(details).not.toHaveAttribute("open");
    await user.tab();
    expect(details.querySelector("summary")).toHaveFocus();
    await user.click(screen.getByText("网络搜索配置"));
    expect(details).toHaveAttribute("open");
    expect(within(details).getByText("OPENCODE_ENABLE_EXA=1 opencode")).toBeVisible();
    expect(within(details).getByText(/Exa/)).toBeVisible();
    expect(within(details).getByText(/尚未检测/)).toBeVisible();
  });

  it.each([
    ["deepseek-harness", "DEEPSEEK_API_KEY"],
    ["openclaw", "OpenClaw"],
    ["nous-hermes-agent", "Hermes"],
    ["claude-desktop", "出口白名单"],
    ["gemini-cli", "Google"],
    ["kimi-code", "FetchURL"],
    ["workbuddy", "WorkBuddy"],
    ["grok-build", "MCP"],
    ["claude-code", "WebSearch"],
  ])("explains the separate search dependency for %s", (agentId, dependency) => {
    render(<AgentCapabilityGuide agentId={agentId} route={route} />);
    expect(screen.getByText("网络搜索配置").closest("details")).toHaveTextContent(dependency);
  });

  it("does not introduce new Codex search behavior or controls", () => {
    render(<AgentCapabilityGuide agentId="codex" route={route} />);
    expect(screen.queryByText("网络搜索配置")).not.toBeInTheDocument();
    expect(screen.queryByRole("checkbox")).not.toBeInTheDocument();
  });
});
