import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ProviderView, SettingsView, StateView } from "./api";
import {
  checkUpgrade,
  discoverProviderModels,
  getPlugins,
  getRouterTable,
  getStats,
  setSettings,
  updateProviderModels,
} from "./api";
import ModelPicker from "./components/ModelPicker";
import ProviderModelManager from "./components/ProviderModelManager";
import About from "./pages/About";
import Plugins from "./pages/Plugins";
import RouterTable from "./pages/RouterTable";
import Settings from "./pages/Settings";
import Stats from "./pages/Stats";

vi.mock("./api", async (loadOriginal) => {
  const original = await loadOriginal<typeof import("./api")>();
  return {
    ...original,
    checkUpgrade: vi.fn(),
    discoverProviderModels: vi.fn(),
    getPlugins: vi.fn(),
    getRouterTable: vi.fn(),
    getStats: vi.fn(),
    setSettings: vi.fn(),
    updateProviderModels: vi.fn(),
  };
});

const settings: SettingsView = {
  listen: "127.0.0.1:8787",
  auth: true,
  metrics: true,
  data_dir: "/data",
  plugins_dir: "/plugins",
  agent: "agent-openai",
  version: "0.1.0",
};

const state: StateView = {
  providers: [],
  tiers: {
    high: { upstream: null, model: null },
    mid: { upstream: null, model: null },
    low: { upstream: null, model: null },
  },
  serve: { phase: "stopped", running: false, listen: settings.listen, virtual_key: null, error: null },
  config_error: null,
  settings,
};

beforeEach(() => {
  vi.mocked(checkUpgrade).mockReset();
  vi.mocked(discoverProviderModels).mockReset();
  vi.mocked(getPlugins).mockReset();
  vi.mocked(getRouterTable).mockReset();
  vi.mocked(getStats).mockReset();
  vi.mocked(setSettings).mockReset();
  vi.mocked(updateProviderModels).mockReset();
  Object.defineProperty(navigator, "clipboard", {
    configurable: true,
    value: { writeText: vi.fn().mockResolvedValue(undefined) },
  });
});

describe("legacy desktop read-only pages", () => {
  it("renders every router layer including configured and missing destinations", async () => {
    vi.mocked(getRouterTable).mockResolvedValue({
      default_pool: "fallback",
      assumed_context_window: 128000,
      threshold: 42,
      rules: [{ match: "tool" }],
      hint_routes: [{ step_type: "code" }],
      bands: [
        { at_least: 80, pool: "high", upstream: "openai", model: "gpt" },
        { at_least: 0, pool: "fallback", upstream: null, model: null },
      ],
      pools: [{ pool: "fallback", upstream: "local", model: null }],
    });
    render(<RouterTable />);
    expect(await screen.findByText("阈值 42")).toBeInTheDocument();
    expect(screen.getByText("openai · gpt")).toBeInTheDocument();
    expect(screen.getByText("— 未配 —")).toBeInTheDocument();
    expect(screen.getByText(/local · \?/)).toBeInTheDocument();
    expect(screen.getByText(/step_type/)).toBeInTheDocument();
  });

  it("renders router empty states and request errors", async () => {
    vi.mocked(getRouterTable).mockResolvedValueOnce({
      default_pool: "",
      assumed_context_window: 0,
      threshold: null,
      rules: [],
      hint_routes: [],
      bands: [],
      pools: [],
    });
    const first = render(<RouterTable />);
    expect(await screen.findByText(/无规则/)).toBeInTheDocument();
    expect(screen.getByText(/还没配档/)).toBeInTheDocument();
    first.unmount();
    vi.mocked(getRouterTable).mockRejectedValueOnce(new Error("router down"));
    render(<RouterTable />);
    expect(await screen.findByText(/router down/)).toBeInTheDocument();
  });

  it("loads grouped stats, changes both filters, and formats nullable cost", async () => {
    vi.mocked(getStats).mockResolvedValue({
      total: {
        requests: 10,
        errors: 2,
        p50_latency_ms: 20,
        p95_latency_ms: 80,
        input_tokens: 100,
        output_tokens: 50,
        cost_micros: 1_250_000,
      },
      groups: [["openai", {
        requests: 10,
        errors: 2,
        p50_latency_ms: 20,
        p95_latency_ms: 80,
        input_tokens: 100,
        output_tokens: 50,
        cost_micros: null,
      }]],
      by: "upstream",
      empty: false,
    });
    const user = userEvent.setup();
    render(<Stats />);
    expect(await screen.findByText("1.2500")).toBeInTheDocument();
    expect(screen.getByText("openai")).toBeInTheDocument();
    const selectors = screen.getAllByRole("combobox");
    await user.selectOptions(selectors[0], "24h");
    await user.selectOptions(selectors[1], "upstream");
    await waitFor(() => expect(getStats).toHaveBeenLastCalledWith("24h", "upstream"));
  });

  it("shows stats empty and error states", async () => {
    vi.mocked(getStats).mockResolvedValueOnce({
      total: {
        requests: 0, errors: 0, p50_latency_ms: 0, p95_latency_ms: 0,
        input_tokens: 0, output_tokens: 0, cost_micros: null,
      },
      groups: [], by: null, empty: true,
    });
    const first = render(<Stats />);
    expect(await screen.findByText(/指标库还没建/)).toBeInTheDocument();
    first.unmount();
    vi.mocked(getStats).mockRejectedValueOnce(new Error("stats down"));
    render(<Stats />);
    expect(await screen.findByText(/stats down/)).toBeInTheDocument();
  });

  it("loads and refreshes plugin metadata, including empty values", async () => {
    vi.mocked(getPlugins)
      .mockResolvedValueOnce({ dir: "/plugins", agent: "agent-openai", dialects: ["openai"], listing: "pkg ok\n" })
      .mockResolvedValueOnce({ dir: "/plugins", agent: "agent-openai", dialects: [], listing: "" });
    const user = userEvent.setup();
    render(<Plugins />);
    expect(await screen.findByText("pkg ok")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "刷新" }));
    expect(await screen.findByText("(空)")).toBeInTheDocument();
    expect(screen.getByText("无")).toBeInTheDocument();
  });

  it("shows plugin errors", async () => {
    vi.mocked(getPlugins).mockRejectedValue(new Error("plugin down"));
    render(<Plugins />);
    expect(await screen.findByText(/plugin down/)).toBeInTheDocument();
  });
});

describe("settings and update actions", () => {
  it("saves changed settings and explains the running-server restart", async () => {
    vi.mocked(setSettings).mockResolvedValue(state);
    const onSaved = vi.fn();
    const user = userEvent.setup();
    render(<Settings settings={settings} serveRunning onSaved={onSaved} />);
    const checks = screen.getAllByRole("checkbox");
    await user.click(checks[0]);
    expect(screen.getByText(/需重启代理/)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "保存" }));
    await waitFor(() => expect(setSettings).toHaveBeenCalledWith(false, true));
    expect(onSaved).toHaveBeenCalledWith(state);
    expect(screen.getByText("已保存 · 重启代理后生效")).toBeInTheDocument();
  });

  it("shows settings save failures", async () => {
    vi.mocked(setSettings).mockRejectedValue(new Error("settings denied"));
    const user = userEvent.setup();
    render(<Settings settings={settings} serveRunning={false} onSaved={vi.fn()} />);
    await user.click(screen.getAllByRole("checkbox")[1]);
    await user.click(screen.getByRole("button", { name: "保存" }));
    expect(await screen.findByText(/settings denied/)).toBeInTheDocument();
  });

  it("reports newer and current releases and copies a release URL", async () => {
    vi.mocked(checkUpgrade)
      .mockResolvedValueOnce({ current: "0.1.0", latest_tag: "v0.2.0", html_url: "https://example.invalid/release", newer: true })
      .mockResolvedValueOnce({ current: "0.2.0", latest_tag: "v0.2.0", html_url: "", newer: false });
    const user = userEvent.setup();
    const writeText = vi.spyOn(navigator.clipboard, "writeText");
    render(<About version="0.1.0" />);
    await user.click(screen.getByRole("button", { name: "检查更新" }));
    expect(await screen.findByText(/有新版本/)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "复制链接" }));
    expect(writeText).toHaveBeenCalledWith("https://example.invalid/release");
    await user.click(screen.getByRole("button", { name: "检查更新" }));
    expect(await screen.findByText(/已是最新/)).toBeInTheDocument();
  });

  it("shows update check failures", async () => {
    vi.mocked(checkUpgrade).mockRejectedValue(new Error("upgrade down"));
    const user = userEvent.setup();
    render(<About version="0.1.0" />);
    await user.click(screen.getByRole("button", { name: "检查更新" }));
    expect(await screen.findByText(/upgrade down/)).toBeInTheDocument();
  });
});

describe("model selection and provider model management", () => {
  it("searches, toggles, refreshes, and adds a custom model by Enter", async () => {
    const models = Array.from({ length: 13 }, (_, index) => `model-${index}`);
    const onToggle = vi.fn();
    const onAdd = vi.fn();
    const onRefresh = vi.fn();
    const user = userEvent.setup();
    render(
      <ModelPicker
        models={[...models, "model-1"]}
        selected={["model-1"]}
        status={{ label: "缓存", tone: "cache", warning: "stale" }}
        onToggle={onToggle}
        onAdd={onAdd}
        onRefresh={onRefresh}
        refreshing={false}
      />,
    );
    expect(screen.getByText("stale")).toBeInTheDocument();
    await user.type(screen.getByLabelText("搜索模型"), "model-12");
    await user.click(screen.getByRole("button", { name: /model-12/ }));
    expect(onToggle).toHaveBeenCalledWith("model-12");
    await user.clear(screen.getByLabelText("搜索模型"));
    await user.type(screen.getByLabelText("搜索模型"), "absent");
    expect(screen.getByText("没有匹配的模型")).toBeInTheDocument();
    await user.type(screen.getByLabelText("手动输入模型 ID"), " custom-model{enter}");
    expect(onAdd).toHaveBeenCalledWith("custom-model");
    await user.click(screen.getByRole("button", { name: "刷新模型" }));
    expect(onRefresh).toHaveBeenCalled();
  });

  it("refreshes and saves provider models, then surfaces both failure paths", async () => {
    const provider: ProviderView = {
      name: "openai", provider: "openai", base_url: "https://api.example/v1",
      models: ["old"], has_auth: true,
    };
    vi.mocked(discoverProviderModels)
      .mockResolvedValueOnce({ models: ["new"], source: "live", fetched_at_ms: 1, warning: null })
      .mockRejectedValueOnce(new Error("catalog down"));
    vi.mocked(updateProviderModels)
      .mockResolvedValueOnce(state)
      .mockRejectedValueOnce(new Error("save down"));
    const onSaved = vi.fn();
    const user = userEvent.setup();
    render(<ProviderModelManager provider={provider} serveRunning onSaved={onSaved} />);
    await user.click(screen.getByRole("button", { name: "刷新模型" }));
    expect(await screen.findByRole("button", { name: /new/ })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /new/ }));
    await user.click(screen.getByRole("button", { name: "保存模型" }));
    await waitFor(() => expect(updateProviderModels).toHaveBeenCalledWith("openai", ["old", "new"]));
    expect(onSaved).toHaveBeenCalledWith(state);
    await user.click(screen.getByRole("button", { name: "刷新模型" }));
    expect(await screen.findByText(/catalog down/)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "保存模型" }));
    expect(await screen.findByText(/save down/)).toBeInTheDocument();
    expect(within(screen.getByText(/代理运行中/).parentElement!).getByRole("button")).toBeEnabled();
  });
});
