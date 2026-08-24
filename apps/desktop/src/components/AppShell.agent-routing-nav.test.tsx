import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
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
  navigationName = "主导航",
  modelCount = 0,
) {
  const onNavigate = vi.fn();
  function ShellHarness() {
    const [modelEntryOpen, setModelEntryOpen] = useState(false);
    return (
      <AppShell
        view={view}
        serve={{ ...serve, ...serveOverride }}
        registry={[]}
        agents={[]}
        commandBusy={false}
        modelCount={modelCount}
        modelEntryOpen={modelEntryOpen}
        onModelEntryOpenChange={setModelEntryOpen}
        onNavigate={onNavigate}
        onToggleServe={vi.fn()}
      >
        <div>content</div>
      </AppShell>
    );
  }
  render(
    <LanguageProvider>
      <ShellHarness />
    </LanguageProvider>,
  );
  return {
    navigation: within(document.querySelector(`[aria-label="${navigationName}"]`)!),
    onNavigate,
  };
}

describe("AppShell Agent and routing navigation", () => {
  it.each([
    ["zh-TW", true, "Agent：已連線"],
    ["zh-TW", false, "Agent：未連線"],
    ["ja", true, "エージェント：接続済み"],
    ["ja", false, "エージェント：未接続"],
  ] as const)("localizes the live connection status for %s", (language, connected, expected) => {
    window.localStorage.setItem("token-station-language", language);
    const navigationName = language === "ja" ? "メインナビゲーション" : "主導覽";
    renderShell("overview", { agent_connected: connected }, navigationName);
    expect(screen.getByTestId("agent-runtime-connection")).toHaveTextContent(expected);
  });

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
    renderShell("providers", {}, "主导航", 1);

    expect(document.querySelector(".station-content"))
      .not.toHaveClass("station-content-overview");
  });

  it("gives Settings its own stable content-scrolling workspace", () => {
    renderShell("settings");

    expect(document.querySelector(".station-content"))
      .toHaveClass("station-content-settings");
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

  it("opens two model setup paths on an empty Models page", async () => {
    const user = userEvent.setup();
    const { navigation, onNavigate } = renderShell("overview");
    await user.click(navigation.getByRole("button", { name: "模型" }));
    const dialog = screen.getByRole("dialog", { name: "选择模型接入方式" });
    expect(within(dialog).getByRole("button", { name: "先选供应商" })).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "先搜模型" })).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "关闭" })).toBeInTheDocument();

    await user.click(within(dialog).getByRole("button", { name: "先搜模型" }));
    expect(onNavigate).toHaveBeenLastCalledWith("add-model");
  });

  it("does not open setup on a configured Models page", () => {
    renderShell("providers", {}, "主导航", 1);

    expect(screen.queryByRole("dialog", { name: "选择模型接入方式" })).toBeNull();
  });
});
