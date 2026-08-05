import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ProviderView, SettingsView, StateView } from "./api";
import {
  checkUpgrade,
  discoverProviderModels,
  getEgress,
  getState,
  getPlugins,
  getRouterTable,
  getStats,
  editProvider,
  previewProviderEndpoints,
  previewProviderRemoval,
  setSettings,
  setProviderModelVision,
  testProvider,
  updateProviderModels,
} from "./api";
import ModelPicker from "./components/ModelPicker";
import ProviderModelManager from "./components/ProviderModelManager";
import ProviderList from "./components/ProviderList";
import About from "./pages/About";
import Plugins from "./pages/Plugins";
import RouterTable from "./pages/RouterTable";
import Settings from "./pages/Settings";
import Stats from "./pages/Stats";

vi.mock("./components/PricingEditor", () => ({ default: () => null }));

vi.mock("./api", async (loadOriginal) => {
  const original = await loadOriginal<typeof import("./api")>();
  return {
    ...original,
    checkUpgrade: vi.fn(),
    discoverProviderModels: vi.fn(),
    getState: vi.fn(),
    getPlugins: vi.fn(),
    getRouterTable: vi.fn(),
    getStats: vi.fn(),
    getEgress: vi.fn().mockResolvedValue({ mode: "direct", proxy_url: null, no_proxy: [], auth_slot: null, routes: [], fixed_direct_classes: ["update_check"] }),
    editProvider: vi.fn(),
    previewProviderEndpoints: vi.fn(),
    previewProviderRemoval: vi.fn(),
    setSettings: vi.fn(),
    setProviderModelVision: vi.fn(),
    testProvider: vi.fn(),
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
  egress_mode: "direct",
  egress_proxy_url: "",
  egress_no_proxy: [],
  egress_auth_username: "",
  egress_auth_slot: "",
};

const state: StateView = {
  providers: [],
  tiers: {
    high: { upstream: null, model: null },
    mid: { upstream: null, model: null },
    low: { upstream: null, model: null },
  },
  keywords: { high: [], mid: [], low: [] },
  agent_routes: {},
  profiles: [],
  local_only: false,
  allow_cloud_fallback: false,
  routing_mode: "tiered",
  quota_accounts: [],
  serve: {
    phase: "stopped", app_runtime: "stopped", listener_reachable: false,
    agent_connected: false, running_revision: null, instance_id: null,
    listen: settings.listen, virtual_key: null, error: null,
  },
  draft_revision: 0,
  saved_revision: 0,
  config_dirty: false,
  config_error: null,
  settings,
};

beforeEach(() => {
  vi.mocked(checkUpgrade).mockReset();
  vi.mocked(discoverProviderModels).mockReset();
  vi.mocked(getEgress).mockReset();
  vi.mocked(getEgress).mockResolvedValue({
    mode: "direct",
    proxy_url: null,
    no_proxy: [],
    auth_slot: null,
    routes: [],
    fixed_direct_classes: ["update_check"],
  });
  vi.mocked(getState).mockReset();
  vi.mocked(getState).mockResolvedValue(state);
  vi.mocked(getPlugins).mockReset();
  vi.mocked(getRouterTable).mockReset();
  vi.mocked(getStats).mockReset();
  vi.mocked(editProvider).mockReset();
  vi.mocked(previewProviderEndpoints).mockReset();
  vi.mocked(previewProviderRemoval).mockReset();
  vi.mocked(setSettings).mockReset();
  vi.mocked(setProviderModelVision).mockReset();
  vi.mocked(testProvider).mockReset();
  vi.mocked(updateProviderModels).mockReset();
  vi.mocked(previewProviderEndpoints).mockResolvedValue({
    chat: "https://api.example/v1/chat/completions",
    responses: "https://api.example/v1/responses",
    messages: "https://api.example/v1/messages",
    loopback: false,
  });
  vi.mocked(getStats).mockResolvedValue({
    total: {
      requests: 0, errors: 0, p50_latency_ms: 0, p95_latency_ms: 0,
      input_tokens: 0, output_tokens: 0, cache_read_tokens: 0, cache_write_tokens: 0,
      reasoning_tokens: 0, cost_micros: null,
      priced_requests: 0, unpriced_requests: 0,
    },
    groups: [], by: "upstream", empty: true,
  });
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
    expect(screen.getByText(/难度词只看最后一条用户消息/)).toBeInTheDocument();
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

  it("loads grouped stats, changes scope and detail view, and formats nullable cost", async () => {
    vi.mocked(getStats).mockResolvedValue({
      total: {
        requests: 10,
        errors: 2,
        p50_latency_ms: 20,
        p95_latency_ms: 80,
        input_tokens: 100,
        output_tokens: 50,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
        reasoning_tokens: 0,
        cost_micros: 1_250_000,
        priced_requests: 10,
        unpriced_requests: 0,
      },
      groups: [["openai", {
        requests: 10,
        errors: 2,
        p50_latency_ms: 20,
        p95_latency_ms: 80,
        input_tokens: 100,
        output_tokens: 50,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
        reasoning_tokens: 0,
        cost_micros: null,
        priced_requests: 0,
        unpriced_requests: 10,
      }]],
      by: "upstream",
      empty: false,
    });
    const user = userEvent.setup();
    render(<Stats />);
    expect(await screen.findByText("1.2500")).toBeInTheDocument();
    expect(screen.getAllByText("openai").length).toBeGreaterThan(0);
    await user.click(screen.getByRole("combobox", { name: "时间范围" }));
    await user.click(within(screen.getByRole("listbox")).getByRole("option", { name: "近 7 天" }));
    await user.click(screen.getByRole("combobox", { name: "供应商过滤" }));
    await user.click(within(screen.getByRole("listbox")).getByRole("option", { name: "openai" }));
    await user.click(screen.getByRole("tab", { name: "供应商" }));
    await waitFor(() => expect(getStats).toHaveBeenCalledWith(
      "7d",
      "upstream",
      null,
      null,
      "openai",
      null,
    ));
  });

  it("shows stats empty and error states", async () => {
    vi.mocked(getStats).mockResolvedValueOnce({
      total: {
        requests: 0, errors: 0, p50_latency_ms: 0, p95_latency_ms: 0,
        input_tokens: 0, output_tokens: 0, cache_read_tokens: 0, cache_write_tokens: 0,
        reasoning_tokens: 0, cost_micros: null,
        priced_requests: 0, unpriced_requests: 0,
      },
      groups: [], by: null, empty: true,
    });
    const first = render(<Stats />);
    expect(await screen.findByText(/还没有本地用量记录/)).toBeInTheDocument();
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
  it("exposes general settings through consistent switch and select controls", () => {
    render(<Settings settings={settings} serveRunning={false} onSaved={vi.fn()} />);

    expect(screen.getByRole("switch", { name: /虚拟 Key 鉴权/ })).toBeChecked();
    expect(screen.getByRole("switch", { name: /本地指标/ })).toBeChecked();
    expect(screen.getByRole("combobox", { name: "出口模式" })).toBeInTheDocument();
  });

  it("renders resolved egress routes in a dedicated full-width list", async () => {
    vi.mocked(getEgress).mockResolvedValue({
      mode: "direct",
      proxy_url: null,
      no_proxy: [],
      auth_slot: null,
      routes: [
        {
          request_class: "provider_request",
          upstream: "deepseek",
          target: "https://api.deepseek.com",
          route: "direct",
          matched_no_proxy: false,
        },
        {
          request_class: "model_catalog",
          upstream: "openrouter",
          target: "https://openrouter.ai/api/v1/models",
          route: "direct",
          matched_no_proxy: false,
        },
      ],
      fixed_direct_classes: ["update_check"],
    });

    render(<Settings settings={settings} serveRunning={false} onSaved={vi.fn()} />);

    const routes = await screen.findByLabelText("实际出口解析");
    expect(routes).toHaveClass("egress-route-list");
    expect(routes).not.toHaveClass("kv-grid");
    expect(within(routes).getByText("provider_request · deepseek")).toBeInTheDocument();
    expect(within(routes).getByText("model_catalog · openrouter")).toBeInTheDocument();
  });

  it("saves changed settings and explains the running-server restart", async () => {
    vi.mocked(setSettings).mockResolvedValue(state);
    const onSaved = vi.fn();
    const user = userEvent.setup();
    render(<Settings settings={settings} serveRunning onSaved={onSaved} />);
    await user.click(screen.getByRole("switch", { name: /虚拟 Key 鉴权/ }));
    expect(screen.getByText(/需重启代理/)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "保存" }));
    await waitFor(() => expect(setSettings).toHaveBeenCalledWith(false, true, {
      egress_mode: "direct",
      egress_proxy_url: "",
      egress_no_proxy: [],
      egress_auth_username: "",
      egress_auth_slot: "",
    }));
    expect(onSaved).toHaveBeenCalledWith(state);
    expect(screen.getByText("已保存 · 重启代理后生效")).toBeInTheDocument();
  });

  it("configures an HTTP egress route with no_proxy and a credential slot", async () => {
    vi.mocked(setSettings).mockResolvedValue(state);
    const user = userEvent.setup();
    render(<Settings settings={settings} serveRunning={false} onSaved={vi.fn()} />);
    await user.click(screen.getByRole("combobox", { name: "出口模式" }));
    await user.click(await screen.findByRole("option", { name: "HTTP CONNECT" }));
    await user.type(screen.getByLabelText("代理 URL"), "http://proxy.internal:8080");
    await user.type(screen.getByLabelText("no_proxy"), "localhost, *.corp.internal");
    await user.type(screen.getByLabelText("代理用户名"), "x");
    await user.type(screen.getByLabelText("代理认证槽"), "proxy_password");
    await user.click(screen.getByRole("button", { name: "保存" }));
    await waitFor(() => expect(setSettings).toHaveBeenCalledWith(true, true, {
      egress_mode: "http",
      egress_proxy_url: "http://proxy.internal:8080",
      egress_no_proxy: ["localhost", "*.corp.internal"],
      egress_auth_username: "x",
      egress_auth_slot: "proxy_password",
    }));
  });

  it("shows settings save failures", async () => {
    vi.mocked(setSettings).mockRejectedValue(new Error("settings denied"));
    const user = userEvent.setup();
    render(<Settings settings={settings} serveRunning={false} onSaved={vi.fn()} />);
    await user.click(screen.getByRole("switch", { name: /本地指标/ }));
    await user.click(screen.getByRole("button", { name: "保存" }));
    expect(await screen.findByText(/settings denied/)).toBeInTheDocument();
  });

  it("focuses and describes the proxy URL for a structured settings error", async () => {
    vi.mocked(setSettings).mockRejectedValue({
      field: "egress_proxy_url",
      reason_code: "invalid_proxy_url",
      message: "代理地址无效",
    });
    const user = userEvent.setup();
    render(<Settings settings={settings} serveRunning={false} onSaved={vi.fn()} />);
    await user.click(screen.getByRole("combobox", { name: "出口模式" }));
    await user.click(await screen.findByRole("option", { name: "HTTP CONNECT" }));
    const proxyUrl = screen.getByLabelText("代理 URL");
    await user.type(proxyUrl, "ftp://invalid.example");
    await user.click(screen.getByRole("button", { name: "保存" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("代理地址无效");
    await waitFor(() => expect(proxyUrl).toHaveFocus());
    expect(proxyUrl).toHaveAttribute("aria-invalid", "true");
    expect(proxyUrl).toHaveAccessibleDescription("代理地址无效");
  });

  it("reports newer and current releases and copies a release URL", async () => {
    vi.mocked(checkUpgrade)
      .mockResolvedValueOnce({ current: "0.1.0", latest_tag: "v0.2.0", html_url: "https://example.invalid/release", newer: true })
      .mockResolvedValueOnce({ current: "0.2.0", latest_tag: "v0.2.0", html_url: "", newer: false });
    const user = userEvent.setup();
    const writeText = vi.spyOn(navigator.clipboard, "writeText");
    render(<About desktopVersion="0.1.0" coreVersion="0.2.0" />);
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
    render(<About desktopVersion="0.1.0" coreVersion="0.2.0" />);
    await user.click(screen.getByRole("button", { name: "检查更新" }));
    expect(await screen.findByText(/upgrade down/)).toBeInTheDocument();
  });

  it("shows an unpublished release as a normal result instead of a GitHub 404", async () => {
    vi.mocked(checkUpgrade).mockResolvedValue({
      status: "no_published_release",
      current: "0.1.0",
      latest_tag: "",
      html_url: "",
      newer: false,
      message: "暂无公开发布版本",
    });
    const user = userEvent.setup();
    render(<About desktopVersion="0.1.0" coreVersion="0.2.0" />);
    await user.click(screen.getByRole("button", { name: "检查更新" }));

    expect(await screen.findByText("暂无公开发布版本")).toBeInTheDocument();
    expect(screen.queryByText(/404/)).not.toBeInTheDocument();
  });
});

describe("model selection and provider model management", () => {
  it("keeps model positions stable when selection changes", async () => {
    const user = userEvent.setup();
    const onToggle = vi.fn();
    const { rerender } = render(
      <ModelPicker
        models={["deepseek-r1", "gemma4:latest", "qwen3:4b-fast"]}
        selected={[]}
        status={{ label: "缓存", tone: "cache" }}
        onToggle={onToggle}
        onAdd={vi.fn()}
        onRefresh={vi.fn()}
        refreshing={false}
      />,
    );

    const getModelOrder = () => Array.from(document.querySelectorAll(".model-chip")).map((item) => item.textContent);
    expect(getModelOrder()).toEqual([
      "+deepseek-r1",
      "+gemma4:latest",
      "+qwen3:4b-fast",
    ]);

    await user.click(screen.getByRole("button", { name: /gemma4:latest/ }));
    expect(onToggle).toHaveBeenCalledWith("gemma4:latest");
    rerender(
      <ModelPicker
        models={["deepseek-r1", "gemma4:latest", "qwen3:4b-fast"]}
        selected={["gemma4:latest"]}
        status={{ label: "缓存", tone: "cache" }}
        onToggle={onToggle}
        onAdd={vi.fn()}
        onRefresh={vi.fn()}
        refreshing={false}
      />,
    );
    expect(getModelOrder()).toEqual([
      "+deepseek-r1",
      "✓gemma4:latest",
      "+qwen3:4b-fast",
    ]);
  });

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
    vi.mocked(getStats).mockResolvedValue({
      total: {
        requests: 3, errors: 1, p50_latency_ms: 10, p95_latency_ms: 20,
        input_tokens: 120, output_tokens: 30, cache_read_tokens: 0, cache_write_tokens: 0,
        reasoning_tokens: 0, cost_micros: 1_250_000,
        priced_requests: 3, unpriced_requests: 0,
      },
      groups: [["openai", {
        requests: 3, errors: 1, p50_latency_ms: 10, p95_latency_ms: 20,
        input_tokens: 120, output_tokens: 30, cache_read_tokens: 0, cache_write_tokens: 0,
        reasoning_tokens: 0, cost_micros: 1_250_000,
        priced_requests: 3, unpriced_requests: 0,
      }]],
      by: "upstream", empty: false,
    });
    const onSaved = vi.fn();
    const user = userEvent.setup();
    render(<ProviderModelManager provider={provider} serveRunning onSaved={onSaved} />);
    expect(await screen.findByText(/150 tokens · 估算成本 1\.2500/)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "刷新模型" }));
    const discovered = await screen.findByRole("button", { name: /new/ });
    expect(discovered).toHaveAttribute("aria-pressed", "false");
    expect(screen.getByRole("button", { name: "✓old" })).toHaveAttribute("aria-pressed", "true");
    expect(updateProviderModels).not.toHaveBeenCalled();
    expect(onSaved).not.toHaveBeenCalled();
    await user.click(discovered);
    await user.click(screen.getByRole("button", { name: "保存模型" }));
    await waitFor(() => expect(updateProviderModels).toHaveBeenCalledWith("openai", ["old", "new"]));
    expect(onSaved).toHaveBeenCalledWith(state);
    await user.click(screen.getByRole("button", { name: "刷新模型" }));
    expect(await screen.findByText(/catalog down/)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "保存模型" }));
    expect(await screen.findByText(/save down/)).toBeInTheDocument();
    expect(within(screen.getByText(/代理运行中/).parentElement!).getByRole("button")).toBeEnabled();
  });

  it("未定价 Provider 显示成本未知而不是零成本", async () => {
    const provider: ProviderView = {
      name: "local", provider: "openai-compatible", base_url: "http://127.0.0.1:11434/v1",
      models: ["local-model"], has_auth: false,
    };
    vi.mocked(getStats).mockResolvedValue({
      total: {
        requests: 1, errors: 0, p50_latency_ms: 5, p95_latency_ms: 5,
        input_tokens: 2, output_tokens: 3, cache_read_tokens: 0, cache_write_tokens: 0,
        reasoning_tokens: 0, cost_micros: null,
        priced_requests: 0, unpriced_requests: 1,
      },
      groups: [["local", {
        requests: 1, errors: 0, p50_latency_ms: 5, p95_latency_ms: 5,
        input_tokens: 2, output_tokens: 3, cache_read_tokens: 0, cache_write_tokens: 0,
        reasoning_tokens: 0, cost_micros: null,
        priced_requests: 0, unpriced_requests: 1,
      }]],
      by: "upstream", empty: false,
    });

    render(<ProviderModelManager provider={provider} serveRunning={false} onSaved={vi.fn()} />);
    expect(await screen.findByText(/5 tokens · 成本未知/)).toBeInTheDocument();
    expect(screen.queryByText(/零成本/)).not.toBeInTheDocument();
  });

  it("shows verified declared unsupported and unknown capability states", () => {
    const provider: ProviderView = {
      name: "matrix",
      provider: "openai-compatible",
      base_url: "https://api.example/v1",
      models: ["model-a", "model-b"],
      model_capabilities: [
        { model: "model-a", tool: "verified", vision: "declared", json_schema: "unsupported" },
        { model: "model-b", tool: "unknown", vision: "unknown", json_schema: "unknown" },
      ],
      has_auth: true,
    };
    render(<ProviderModelManager provider={provider} serveRunning={false} onSaved={vi.fn()} />);
    expect(screen.getByText("工具 · 已验证")).toBeInTheDocument();
    expect(screen.getByText("视觉 · 已声明")).toBeInTheDocument();
    expect(screen.getByText("JSON · 不支持")).toBeInTheDocument();
    expect(screen.getAllByText(/· 未知/)).toHaveLength(3);
  });

  it("allows an unknown model to be declared vision-capable", async () => {
    const provider: ProviderView = {
      name: "matrix",
      provider: "openai-compatible",
      base_url: "https://api.example/v1",
      models: ["model-a"],
      model_capabilities: [
        { model: "model-a", tool: "unknown", vision: "unknown", json_schema: "unknown" },
      ],
      has_auth: true,
    };
    vi.mocked(setProviderModelVision).mockResolvedValue(state);
    const onSaved = vi.fn();
    const user = userEvent.setup();

    render(<ProviderModelManager provider={provider} serveRunning={false} onSaved={onSaved} />);
    await user.click(screen.getByRole("button", { name: "为 model-a 声明视觉支持" }));

    await waitFor(() =>
      expect(setProviderModelVision).toHaveBeenCalledWith("matrix", "model-a", true),
    );
    expect(onSaved).toHaveBeenCalledWith(state);
  });

  it("保留已下架模型并展示本次目录差异", async () => {
    const provider: ProviderView = {
      name: "catalog", provider: "openai-compatible", base_url: "https://api.example/v1",
      models: ["old-model"], has_auth: true, catalog_revision: 1,
      catalog: [{
        model: "old-model", tool: "unknown", vision: "unknown", json_schema: "unknown",
        source: "live", last_seen_ms: 1, catalog_state: "active",
      }],
    };
    vi.mocked(discoverProviderModels).mockResolvedValue({
      models: ["new-model"], source: "live", fetched_at_ms: 2, warning: null,
      revision: 2, added: ["new-model"], removed: ["old-model"],
      catalog: [
        {
          model: "new-model", tool: "unknown", vision: "unknown", json_schema: "unknown",
          source: "live", last_seen_ms: 2, catalog_state: "active",
        },
        {
          model: "old-model", tool: "unknown", vision: "unknown", json_schema: "unknown",
          source: "live", last_seen_ms: 1, catalog_state: "removed",
        },
      ],
    });
    const user = userEvent.setup();
    render(<ProviderModelManager provider={provider} serveRunning={false} onSaved={vi.fn()} />);
    await user.click(screen.getByRole("button", { name: "刷新模型" }));
    expect(await screen.findByText((_, element) =>
      Boolean(
        element?.classList.contains("removed")
        && element.textContent?.includes("old-model")
        && element.textContent.includes("仍保留引用"),
      ),
    )).toBeInTheDocument();
    expect(screen.getByText((_, element) =>
      Boolean(element?.classList.contains("added") && element.textContent?.includes("new-model")),
    )).toBeInTheDocument();
    expect(screen.getByText(/已下架/)).toBeInTheDocument();
  });

  it("refreshes public state after trusted model capabilities are synchronized", async () => {
    const provider: ProviderView = {
      name: "openrouter",
      provider: "openai-compatible",
      base_url: "https://openrouter.ai/api/v1",
      models: ["vision-model"],
      model_capabilities: [{
        model: "vision-model", tool: "unknown", vision: "unknown", json_schema: "unknown",
      }],
      has_auth: true,
    };
    vi.mocked(discoverProviderModels).mockResolvedValue({
      models: ["vision-model"],
      source: "live",
      fetched_at_ms: 42,
      warning: null,
      capabilities_updated: true,
      catalog: [{
        model: "vision-model",
        tool: "unknown",
        vision: "verified",
        json_schema: "unknown",
        source: "live",
        last_seen_ms: 42,
        catalog_state: "active",
      }],
    });
    const onSaved = vi.fn();
    const user = userEvent.setup();

    render(<ProviderModelManager provider={provider} serveRunning={false} onSaved={onSaved} />);
    await user.click(screen.getByRole("button", { name: "刷新模型" }));

    await waitFor(() => expect(getState).toHaveBeenCalled());
    expect(onSaved).toHaveBeenCalledWith(state);
  });

  it("展示 Provider 八层耗时、最近测试时间和绿色健康徽章", async () => {
    const provider: ProviderView = {
      name: "tested", provider: "openai-compatible", base_url: "https://api.example/v1",
      models: ["model-a"], has_auth: true,
    };
    vi.mocked(testProvider).mockResolvedValue([{
      model: "model-a",
      stages: [
        "network", "http", "auth", "model", "generation", "stream", "tool", "json",
      ].map((layer, index) => ({
        layer: layer as "network" | "http" | "auth" | "model" | "generation" | "stream" | "tool" | "json",
        status: "pass" as const,
        duration_ms: index < 5 ? 37 : index + 10,
        timing_kind: index < 5 ? "cumulative" as const : "stage" as const,
      })),
    }]);
    const user = userEvent.setup();
    render(<ProviderModelManager provider={provider} serveRunning={false} onSaved={vi.fn()} />);
    await user.click(screen.getByRole("button", { name: "运行分层测试" }));
    expect(await screen.findByText("流式 · pass · 15ms")).toBeInTheDocument();
    expect(screen.getByText("Tool · pass · 16ms")).toBeInTheDocument();
    expect(screen.getByText("JSON Schema · pass · 17ms")).toBeInTheDocument();
    expect(screen.getByText("DNS / 网络 · pass · ≤37ms")).toBeInTheDocument();
    expect(screen.getByLabelText("Provider 健康状态：健康")).toBeInTheDocument();
    expect(screen.getByText(/最近测试/)).toBeInTheDocument();
    expect(screen.queryByText(/未执行/)).not.toBeInTheDocument();
  });

  it("基础链路失败为红色，能力层失败为黄色", async () => {
    const provider: ProviderView = {
      name: "health", provider: "openai-compatible", base_url: "https://api.example/v1",
      models: ["model-a"], has_auth: true,
    };
    vi.mocked(testProvider)
      .mockResolvedValueOnce([{
        model: "model-a",
        stages: [
          { layer: "network", status: "pass", duration_ms: 12, timing_kind: "cumulative" },
          { layer: "http", status: "pass", duration_ms: 12, timing_kind: "cumulative" },
          { layer: "auth", status: "fail", duration_ms: 12, timing_kind: "cumulative", detail: "401" },
          { layer: "model", status: "skipped" },
          { layer: "generation", status: "skipped" },
          { layer: "stream", status: "skipped" },
          { layer: "tool", status: "skipped" },
          { layer: "json", status: "skipped" },
        ],
      }])
      .mockResolvedValueOnce([{
        model: "model-a",
        stages: [
          ...["network", "http", "auth", "model", "generation"].map((layer) => ({
            layer: layer as "network" | "http" | "auth" | "model" | "generation",
            status: "pass" as const,
            duration_ms: 20,
            timing_kind: "cumulative" as const,
          })),
          { layer: "stream", status: "pass", duration_ms: 4, timing_kind: "stage" },
          { layer: "tool", status: "fail", duration_ms: 5, timing_kind: "stage", detail: "unsupported" },
          { layer: "json", status: "pass", duration_ms: 6, timing_kind: "stage" },
        ],
      }]);
    const user = userEvent.setup();
    render(<ProviderModelManager provider={provider} serveRunning={false} onSaved={vi.fn()} />);

    await user.click(screen.getByRole("button", { name: "运行分层测试" }));
    expect(await screen.findByLabelText("Provider 健康状态：不可用")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "运行分层测试" }));
    expect(await screen.findByLabelText("Provider 健康状态：能力退化")).toBeInTheDocument();
  });
});

describe("provider deletion lifecycle", () => {
  it("删除前展示路由引用并禁止确认", async () => {
    const provider: ProviderView = {
      name: "referenced", provider: "openai-compatible", base_url: "https://api.example/v1",
      models: ["model-a"], has_auth: true,
    };
    vi.mocked(previewProviderRemoval).mockResolvedValue({
      name: "referenced",
      references: ["主页/上档#1", "Agent/codex/high"],
      can_remove: false,
    });
    const user = userEvent.setup();
    render(
      <ProviderList
        providers={[provider]}
        deletedProviders={[]}
        recoveryError={null}
        serveRunning={false}
        busy={false}
        onRemove={vi.fn()}
        onRestore={vi.fn()}
        onStateChange={vi.fn()}
      />,
    );
    await user.click(screen.getByRole("button", { name: "删除" }));
    expect(await screen.findByText("主页/上档#1")).toBeInTheDocument();
    expect(screen.getByText("Agent/codex/high")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "确认移入回收站" })).toBeDisabled();
  });
});
