import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { AgentUiMetadataView, ServeView, SettingsView } from "../api";
import { ErrorToastProvider } from "../components/ErrorToast";
import SettingsHub from "./SettingsHub";

vi.mock("../api", async (loadOriginal) => {
  const original = await loadOriginal<typeof import("../api")>();
  return {
    ...original,
    getEgress: vi.fn().mockResolvedValue({
      mode: "direct",
      proxy_url: null,
      no_proxy: [],
      auth_slot: null,
      routes: [],
      fixed_direct_classes: ["update_check"],
    }),
  };
});

const settings: SettingsView = {
  listen: "127.0.0.1:8787",
  auth: true,
  metrics: true,
  data_dir: "/data",
  plugins_dir: "/plugins",
  agent: "codex",
  version: "1.1.3",
  egress_mode: "direct",
  egress_proxy_url: "",
  egress_no_proxy: [],
  egress_auth_username: "",
  egress_auth_slot: "",
};

const serve: ServeView = {
  phase: "running",
  app_runtime: "running",
  listener_reachable: true,
  agent_connected: true,
  running_revision: 1,
  instance_id: "instance",
  listen: settings.listen,
  virtual_key: "vk-test-secret",
  error: null,
};

const registry: AgentUiMetadataView[] = [];

describe("SettingsHub clipboard feedback", () => {
  it("moves and activates Settings sections with vertical navigation keys", async () => {
    const user = userEvent.setup();
    render(
      <ErrorToastProvider>
        <SettingsHub
          settings={settings}
          serve={serve}
          registry={registry}
          visibleAgentIds={new Set()}
          onAgentVisibilityChange={vi.fn()}
          onOpenFirstRunGuide={vi.fn()}
          onSaved={vi.fn()}
        />
      </ErrorToastProvider>,
    );

    const general = screen.getByRole("button", { name: /通用/ });
    const agentVisibility = screen.getByRole("button", { name: /Agent 显示/ });
    const about = screen.getByRole("button", { name: /关于/ });

    general.focus();
    await user.keyboard("{ArrowDown}");
    expect(agentVisibility).toHaveFocus();
    expect(agentVisibility).toHaveAttribute("aria-current", "page");
    expect(screen.getByRole("heading", { name: "Agent 显示" })).toBeInTheDocument();

    await user.keyboard("{ArrowUp}");
    expect(general).toHaveFocus();
    expect(general).toHaveAttribute("aria-current", "page");

    await user.keyboard("{ArrowUp}");
    expect(about).toHaveFocus();
    expect(about).toHaveAttribute("aria-current", "page");

    await user.keyboard("{Home}");
    expect(general).toHaveFocus();
    await user.keyboard("{End}");
    expect(about).toHaveFocus();
  });

  it("switches Settings categories without entrance motion", async () => {
    const user = userEvent.setup();
    const cancel = vi.fn();
    const animate = vi.fn().mockReturnValue({ cancel } as unknown as Animation);
    const originalAnimate = HTMLElement.prototype.animate;
    Object.defineProperty(HTMLElement.prototype, "animate", {
      configurable: true,
      value: animate,
    });

    try {
      render(
        <ErrorToastProvider>
          <SettingsHub
            settings={settings}
            serve={serve}
            registry={registry}
            visibleAgentIds={new Set()}
            onAgentVisibilityChange={vi.fn()}
            onOpenFirstRunGuide={vi.fn()}
            onSaved={vi.fn()}
          />
        </ErrorToastProvider>,
      );
      expect(animate).not.toHaveBeenCalled();

      await user.click(screen.getByRole("button", { name: /Agent 显示/ }));

      expect(await screen.findByRole("heading", { name: "Agent 显示" })).toBeInTheDocument();
      expect(animate).not.toHaveBeenCalled();
    } finally {
      if (originalAnimate) {
        Object.defineProperty(HTMLElement.prototype, "animate", {
          configurable: true,
          value: originalAnimate,
        });
      } else {
        Reflect.deleteProperty(HTMLElement.prototype, "animate");
      }
    }
  });

  it("复制虚拟 API Key 失败时只在左下角提示且不暴露密钥", async () => {
    const user = userEvent.setup();
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText: vi.fn().mockRejectedValue(new Error("clipboard denied")) },
    });
    render(
      <ErrorToastProvider>
        <SettingsHub
          settings={settings}
          serve={serve}
          registry={registry}
          visibleAgentIds={new Set()}
          onAgentVisibilityChange={vi.fn()}
          onOpenFirstRunGuide={vi.fn()}
          onSaved={vi.fn()}
        />
      </ErrorToastProvider>,
    );

    await user.click(screen.getByRole("button", { name: "复制" }));

    const message = "无法复制虚拟 API Key。请检查系统剪贴板权限，然后重试。";
    expect(await within(screen.getByTestId("error-toast-viewport")).findByRole("alert"))
      .toHaveTextContent(message);
    expect(screen.getByRole("button", { name: "复制" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "已复制" })).not.toBeInTheDocument();
    expect(screen.queryByText("vk-test-secret")).not.toBeInTheDocument();
    expect(screen.queryByText(message, { selector: ".settings-card .banner" }))
      .not.toBeInTheDocument();
  });
});
