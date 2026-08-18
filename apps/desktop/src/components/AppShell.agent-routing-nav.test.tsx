import { cleanup, render, screen, within } from "@testing-library/react";
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

function renderShell(view: Parameters<typeof AppShell>[0]["view"]) {
  render(
    <LanguageProvider>
      <AppShell
        view={view}
        serve={serve}
        registry={[]}
        agents={[]}
        commandBusy={false}
        onNavigate={vi.fn()}
        onToggleServe={vi.fn()}
      >
        <div>content</div>
      </AppShell>
    </LanguageProvider>,
  );
  return within(screen.getByRole("navigation", { name: "主导航" }));
}

describe("AppShell Agent and routing navigation", () => {
  it("exposes Agent and routing as separate primary pages", () => {
    const navigation = renderShell("agents");

    expect(navigation.getByRole("button", { name: "Agent" })).toHaveAttribute("aria-current", "page");
    expect(navigation.getByRole("button", { name: "路由" })).not.toHaveAttribute("aria-current");
    expect(navigation.getByRole("button", { name: "供应商" })).toBeInTheDocument();
    expect(navigation.getByRole("button", { name: "用量" })).toBeInTheDocument();
    expect(navigation.queryByRole("button", { name: "主页" })).toBeNull();
  });

  it("maps connection details to Agent and route details to routing", () => {
    let navigation = renderShell("agent:claude-code");
    expect(navigation.getByRole("button", { name: "Agent" })).toHaveAttribute("aria-current", "page");

    cleanup();
    navigation = renderShell("agent-route:claude-code");
    expect(navigation.getByRole("button", { name: "路由" })).toHaveAttribute("aria-current", "page");
  });
});
