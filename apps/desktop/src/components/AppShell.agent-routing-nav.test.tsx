import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { ServeView } from "../api";
import AppShell from "./AppShell";
import { LanguageProvider } from "./LanguageProvider";

const serve: ServeView = {
  phase: "stopped",
  app_runtime: "stopped",
  listener_reachable: false,
  agent_connected: false,
  listen: "127.0.0.1:4100",
  virtual_key: null,
  error: null,
  running_revision: null,
  instance_id: null,
};

function renderShell(
  view: Parameters<typeof AppShell>[0]["view"],
  serveOverride: Partial<ServeView> = {},
) {
  const onNavigate = vi.fn();
  render(
    <LanguageProvider>
      <AppShell
        view={view}
        serve={{ ...serve, ...serveOverride }}
        registry={[]}
        agents={[]}
        commandBusy={false}
        onNavigate={onNavigate}
        onToggleServe={vi.fn()}
      >
        <div>content</div>
      </AppShell>
    </LanguageProvider>,
  );
  return {
    navigation: within(screen.getByRole("navigation", { name: "主导航" })),
    onNavigate,
  };
}

describe("AppShell Agent and routing navigation", () => {
  it("exposes Agent and routing as separate primary pages", () => {
    const { navigation } = renderShell("agents");

    expect(navigation.getByRole("button", { name: "主页" })).not.toHaveAttribute("aria-current");
    expect(navigation.getByRole("button", { name: "Agent" })).toHaveAttribute("aria-current", "page");
    expect(navigation.getByRole("button", { name: "路由" })).not.toHaveAttribute("aria-current");
    expect(navigation.getByRole("button", { name: "模型" })).toBeInTheDocument();
    expect(navigation.getByRole("button", { name: "用量" })).toBeInTheDocument();
  });

  it("marks Home as the current primary page on Overview", () => {
    const { navigation } = renderShell("overview");

    expect(navigation.getByRole("button", { name: "主页" })).toHaveAttribute("aria-current", "page");
    expect(navigation.getByRole("button", { name: "Agent" })).not.toHaveAttribute("aria-current");
    expect(document.querySelector(".station-content"))
      .toHaveClass("station-content-overview");
  });

  it("does not lock scrolling for non-overview workspaces", () => {
    renderShell("providers");

    expect(document.querySelector(".station-content"))
      .not.toHaveClass("station-content-overview");
  });

  it("maps connection details to Agent and route details to routing", () => {
    let { navigation } = renderShell("agent:claude-code");
    expect(navigation.getByRole("button", { name: "Agent" })).toHaveAttribute("aria-current", "page");

    cleanup();
    ({ navigation } = renderShell("agent-route:claude-code"));
    expect(navigation.getByRole("button", { name: "路由" })).toHaveAttribute("aria-current", "page");
  });

  it("keeps runtime revision metadata out of the compact button content", () => {
    renderShell("overview", {
      phase: "running",
      app_runtime: "running",
      listener_reachable: true,
      running_revision: 141,
    });

    const runtimeButton = screen.getByRole("button", { name: /代理运行中.*停止/ });
    expect(runtimeButton).toHaveAttribute("title", expect.stringContaining("rev 141"));
    expect(within(runtimeButton).queryByText("rev 141")).not.toBeInTheDocument();
  });

  it("opens two model setup paths from the Models navigation item", async () => {
    const user = userEvent.setup();
    const { navigation, onNavigate } = renderShell("overview");

    await user.click(navigation.getByRole("button", { name: "模型" }));

    expect(onNavigate).toHaveBeenCalledWith("providers");
    const dialog = screen.getByRole("dialog", { name: "选择模型接入方式" });
    expect(within(dialog).getByRole("button", { name: "先选供应商" })).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "先搜模型" })).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "关闭" })).toBeInTheDocument();

    await user.click(within(dialog).getByRole("button", { name: "先搜模型" }));
    expect(onNavigate).toHaveBeenLastCalledWith("add-model");
  });
});
