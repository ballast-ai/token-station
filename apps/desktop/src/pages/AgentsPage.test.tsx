import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { AgentUiMetadataView, AgentView } from "../api";
import { LanguageProvider } from "../components/LanguageProvider";
import AgentsPage from "./AgentsPage";

const registry: AgentUiMetadataView[] = [
  { agent_id: "claude-code", legacy_kind: "cc", display_name: "Claude Code", icon_key: "claude", admission: "supported", nav_mark: "C" },
  { agent_id: "kimi-code", legacy_kind: null, display_name: "Kimi Code", icon_key: "kimi", admission: "supported", nav_mark: "K" },
];
const agents: AgentView[] = registry.map((metadata) => ({
  metadata,
  installations: [],
  status: "DETECTED_VERIFIED",
  catalog_sequence: 1,
  catalog_expires_at_ms: null,
  catalog_source: "builtin",
  catalog_warning: null,
}));

function renderPage(mode: "connections" | "routing") {
  const onOpenAgent = vi.fn();
  render(
    <LanguageProvider>
      <AgentsPage
        mode={mode}
        registry={registry}
        agents={agents}
        revealingAgentIds={new Set()}
        selectedAgentId="claude-code"
        homeSelected={mode === "routing"}
        scanBusy={false}
        onOpenHome={vi.fn()}
        onOpenAgent={onOpenAgent}
        onRescan={vi.fn()}
      >
        <div>selected detail</div>
      </AgentsPage>
    </LanguageProvider>,
  );
  return onOpenAgent;
}

describe("AgentsPage split workspaces", () => {
  it("uses a single-select detected Agent list without global routing", async () => {
    const user = userEvent.setup();
    const onOpenAgent = renderPage("connections");
    const selector = screen.getByRole("region", { name: "Agent 选择列表" });

    expect(screen.getByRole("heading", { name: "Agent 接入" })).toBeInTheDocument();
    expect(within(selector).getByRole("heading", { name: "发现 Agents" })).toBeInTheDocument();
    expect(within(selector).queryByRole("button", { name: "全局路由" })).toBeNull();
    expect(within(selector).getByRole("button", { name: "Claude Code" })).toHaveAttribute("aria-current", "page");

    await user.click(within(selector).getByRole("button", { name: "Kimi Code" }));
    expect(onOpenAgent).toHaveBeenCalledWith("kimi-code");
  });

  it("keeps global routing visible and reveals every scanned Agent from one button", async () => {
    const user = userEvent.setup();
    const onOpenAgent = renderPage("routing");
    const selector = screen.getByRole("region", { name: "路由范围" });

    expect(screen.getByRole("heading", { name: "路由配置" })).toBeInTheDocument();
    expect(within(selector).getByRole("button", { name: "全局路由" })).toBeVisible();
    expect(within(selector).getByRole("button", { name: "企业路由" })).toBeVisible();
    expect(within(selector).queryByRole("button", { name: "重新扫描" })).toBeNull();
    expect(within(selector).queryByText("所有 Agent 的默认策略")).toBeNull();
    const disclosure = within(selector).getByRole("button", { name: "Agent 路由" });
    expect(disclosure).toHaveAttribute("aria-expanded", "false");
    expect(within(selector).queryByRole("button", { name: "Claude Code" })).toBeNull();

    await user.click(disclosure);
    expect(disclosure).toHaveAttribute("aria-expanded", "true");
    expect(within(selector).getByRole("button", { name: "Claude Code" })).toBeVisible();
    expect(within(selector).getByRole("button", { name: "Kimi Code" })).toBeVisible();
    expect(within(selector).getByRole("button", { name: "Claude Code" })
      .querySelector('[data-agent-brand="claude-code"]')).toBeInTheDocument();

    await user.click(within(selector).getByRole("button", { name: "Kimi Code" }));
    expect(onOpenAgent).toHaveBeenCalledWith("kimi-code");
    expect(disclosure).toHaveAttribute("aria-expanded", "true");
    expect(within(selector).getByRole("button", { name: "Claude Code" })).toBeVisible();
  });
});
