import { invoke } from "@tauri-apps/api/core";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import App from "./App";
import type { StateView } from "./api";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const invokeMock = vi.mocked(invoke);

const stateFixture: StateView = {
  providers: [],
  tiers: {
    high: { upstream: null, model: null },
    mid: { upstream: null, model: null },
    low: { upstream: null, model: null },
  },
  serve: { running: false, listen: "127.0.0.1:8787", virtual_key: null },
  config_error: null,
  settings: {
    listen: "127.0.0.1:8787",
    auth: true,
    metrics: true,
    data_dir: "/tmp/token-station-test/data",
    plugins_dir: "/tmp/token-station-test/plugins",
    agent: "test-agent",
    version: "test-version",
  },
};

beforeEach(() => {
  invokeMock.mockImplementation(async (command) => {
    if (command === "get_state") return stateFixture;
    if (command === "scan_agents") return [];
    throw new Error(`unexpected IPC command: ${command}`);
  });
});

describe("dynamic Agent navigation", () => {
  it("replaces the static three-button bar with the structured Agents page", async () => {
    const user = userEvent.setup();
    const { container } = render(<App />);
    await screen.findByText("token-station");
    expect(container.querySelector(".agentbar")).toBeNull();

    await user.click(screen.getByRole("button", { name: "Agents" }));
    await screen.findByRole("heading", { name: "Agent 管理" });
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("scan_agents"));
  });

  it("starts and stops the proxy and saves the editable configuration", async () => {
    const running = {
      ...stateFixture,
      serve: { running: true, listen: "127.0.0.1:8787", virtual_key: "vk-test" },
    } satisfies StateView;
    invokeMock.mockImplementation(async (command) => {
      if (command === "get_state" || command === "serve_stop" || command === "save_config") {
        return stateFixture;
      }
      if (command === "serve_start") return running;
      if (command === "scan_agents") return [];
      throw new Error(`unexpected IPC command: ${command}`);
    });
    const user = userEvent.setup();
    render(<App />);
    await user.click(await screen.findByRole("button", { name: "启动代理" }));
    expect(await screen.findByText("vk-test")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "停止" }));
    expect(await screen.findByRole("button", { name: "启动代理" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "保存并应用" }));
    expect(await screen.findByText("已保存并校验")).toBeInTheDocument();
  });

  it("discovers preset models and submits only the structured provider form", async () => {
    invokeMock.mockImplementation(async (command) => {
      if (command === "get_state" || command === "add_provider") return stateFixture;
      if (command === "scan_agents") return [];
      if (command === "discover_provider_models") {
        return { models: ["gpt-new"], source: "live", fetched_at_ms: 1, warning: null };
      }
      throw new Error(`unexpected IPC command: ${command}`);
    });
    const user = userEvent.setup();
    render(<App />);
    await screen.findByText("token-station");
    await user.selectOptions(screen.getAllByRole("combobox")[6], "openai");
    expect(screen.getByText("https://api.openai.com/v1")).toBeInTheDocument();
    await user.type(screen.getByPlaceholderText("API Key"), "secret-test");
    await user.click(screen.getByRole("button", { name: "刷新模型" }));
    expect(await screen.findByText("已同步 1 个")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "添加" }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("add_provider", {
      name: "openai",
      baseUrl: "https://api.openai.com/v1",
      models: ["gpt-5.5", "gpt-5.5-mini", "gpt-4.1", "o4-mini"],
      apiKey: "secret-test",
    }));
    expect(screen.getByText("供应商已添加")).toBeInTheDocument();
  });
});
