import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { AgentRouteView } from "../api";
import AgentCapabilityGuide from "./AgentCapabilityGuide";

const route: AgentRouteView = {
  mode: "inherit", inherits_global: true, routing_mode: "direct",
  direct_target: { upstream: "wecoding", model: "glm-5.2" },
  tiers: { high: { upstream: null, model: null }, mid: { upstream: null, model: null }, low: { upstream: null, model: null } },
  config_error: null, profile: null,
};

describe("Agent route guidance", () => {
  it("shows the effective direct target even before an Agent connects", () => {
    render(<AgentCapabilityGuide route={route} />);
    expect(screen.getByText("跟随全局")).toBeInTheDocument();
    expect(screen.getByText("wecoding / glm-5.2")).toBeInTheDocument();
    expect(screen.queryByText(/网关正常|搜索可用|搜索已通过/)).not.toBeInTheDocument();
  });

  it("does not mistake inherited tiers for full route inheritance", () => {
    render(<AgentCapabilityGuide route={{
      ...route, inherits_global: false, direct_target: { upstream: "kimi", model: "kimi-k3" },
    }} />);
    expect(screen.getByText("独立路由")).toBeInTheDocument();
    expect(screen.getByText("kimi / kimi-k3")).toBeInTheDocument();
    expect(screen.queryByText("跟随全局")).not.toBeInTheDocument();
  });

  it("does not claim a target when a direct configuration is incomplete", () => {
    render(<AgentCapabilityGuide route={{ ...route, direct_target: null }} />);
    expect(screen.getByText("尚未选择供应商和模型")).toBeInTheDocument();
  });

});
