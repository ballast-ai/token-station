import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ProviderView, SettingsView, StateView } from "./api";
import {
  checkDesktopUpdate,
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
  setProviderModelLimits,
  testProvider,
  updateProviderModels,
} from "./api";
import ModelPicker from "./components/ModelPicker";
import ProviderModelManager, { formatTestedAt } from "./components/ProviderModelManager";
import ProviderList from "./components/ProviderList";
import { ErrorToastProvider } from "./components/ErrorToast";
import About from "./pages/About";
import Plugins from "./pages/Plugins";
import RouterTable from "./pages/RouterTable";
import Settings from "./pages/Settings";
import Stats from "./pages/Stats";

vi.mock("./components/PricingEditor", () => ({ default: () => null }));

describe("localized provider timestamps", () => {
  it.each(["zh-TW", "ja"] as const)("passes %s to Intl formatting", (language) => {
    const format = vi.spyOn(Date.prototype, "toLocaleString").mockReturnValue("localized date");
    try {
      expect(formatTestedAt(1_752_000_000_000, language)).toBe("localized date");
      expect(format).toHaveBeenCalledWith(language);
    } finally {
      format.mockRestore();
    }
  });
});

vi.mock("./api", async (loadOriginal) => {
  const original = await loadOriginal<typeof import("./api")>();
  return {
    ...original,
    checkDesktopUpdate: vi.fn(),
    installDesktopUpdateAndRestart: vi.fn(),
    listenDesktopUpdateProgress: vi.fn().mockResolvedValue(() => undefined),
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
    setProviderModelLimits: vi.fn(),
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
  vi.mocked(checkDesktopUpdate).mockReset();
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
    expect(await screen.findByText("操作未能完成。请重试；如果仍然失败，请从自救模式打开本地日志。")).toBeInTheDocument();
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
    await user.click(screen.getByRole("button", { name: "筛选" }));
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
    expect((await screen.findAllByText("操作未能完成。请重试；如果仍然失败，请从自救模式打开本地日志。")).length).toBeGreaterThan(0);
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
    expect(await screen.findByText("操作未能完成。请重试；如果仍然失败，请从自救模式打开本地日志。")).toBeInTheDocument();
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
    render(
      <ErrorToastProvider>
        <Settings settings={settings} serveRunning onSaved={onSaved} />
      </ErrorToastProvider>,
    );
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
    const viewport = screen.getByTestId("error-toast-viewport");
    expect(await within(viewport).findByRole("status"))
      .toHaveTextContent("已保存 · 重启代理后生效");
    expect(screen.queryByText("已保存 · 重启代理后生效", { selector: ".settings-card .banner" }))
      .toBeNull();
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
    render(
      <ErrorToastProvider>
        <Settings settings={settings} serveRunning={false} onSaved={vi.fn()} />
      </ErrorToastProvider>,
    );
    await user.click(screen.getByRole("switch", { name: /本地指标/ }));
    await user.click(screen.getByRole("button", { name: "保存" }));
    const viewport = screen.getByTestId("error-toast-viewport");
    expect(await within(viewport).findByText("操作未能完成。请重试；如果仍然失败，请从自救模式打开本地日志。"))
      .toBeInTheDocument();
    expect(screen.queryByText("操作未能完成。请重试；如果仍然失败，请从自救模式打开本地日志。", { selector: ".settings-card .banner" }))
      .toBeNull();
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

    expect(await screen.findByRole("alert"))
      .toHaveTextContent("地址格式不正确。请输入完整的 HTTP、HTTPS 或 SOCKS5 地址，然后重试。");
    expect(screen.queryByText("地址格式不正确。请输入完整的 HTTP、HTTPS 或 SOCKS5 地址，然后重试。", { selector: ".settings-card .banner" }))
      .toBeNull();
    await waitFor(() => expect(proxyUrl).toHaveFocus());
    expect(proxyUrl).toHaveAttribute("aria-invalid", "true");
    expect(proxyUrl).toHaveAccessibleDescription("地址格式不正确。请输入完整的 HTTP、HTTPS 或 SOCKS5 地址，然后重试。");
  });

  it("reports newer and current releases and copies a release URL", async () => {
    vi.mocked(checkDesktopUpdate)
      .mockResolvedValueOnce({ status: "update_available", current_version: "0.1.0", version: "0.2.0", notes: null, pub_date: null, release_url: "https://example.invalid/release", message: null })
      .mockResolvedValueOnce({ status: "up_to_date", current_version: "0.2.0", version: null, notes: null, pub_date: null, release_url: "", message: null });
    const user = userEvent.setup();
    const writeText = vi.spyOn(navigator.clipboard, "writeText");
    const first = render(<About desktopVersion="0.1.0" coreVersion="0.2.0" />);
    await user.click(screen.getByRole("button", { name: "检查更新" }));
    expect(await screen.findByText(/发现新版本/)).toBeInTheDocument();
    await user.click(await screen.findByRole("button", { name: "取消" }));
    await user.click(screen.getByRole("button", { name: "复制链接" }));
    expect(writeText).toHaveBeenCalledWith("https://example.invalid/release");
    first.unmount();
    render(<About desktopVersion="0.2.0" coreVersion="0.2.0" />);
    await user.click(screen.getByRole("button", { name: "检查更新" }));
    expect(await screen.findByText(/已是最新/)).toBeInTheDocument();
  });

  it("shows update check failures", async () => {
    vi.mocked(checkDesktopUpdate).mockRejectedValue(new Error("upgrade down"));
    const user = userEvent.setup();
    render(
      <ErrorToastProvider>
        <About desktopVersion="0.1.0" coreVersion="0.2.0" />
      </ErrorToastProvider>,
    );
    await user.click(screen.getByRole("button", { name: "检查更新" }));
    expect(await within(screen.getByTestId("error-toast-viewport")).findByText(
      "Token Station 无法检查更新。请检查网络连接，稍后重试。",
    )).toBeInTheDocument();
  });

  it("shows an unpublished release as a normal result instead of a GitHub 404", async () => {
    vi.mocked(checkDesktopUpdate).mockResolvedValue({
      status: "unavailable",
      current_version: "0.1.0",
      version: null,
      notes: null,
      pub_date: null,
      release_url: "",
      message: "暂无公开发布版本",
    });
    const user = userEvent.setup();
    render(<About desktopVersion="0.1.0" coreVersion="0.2.0" />);
    await user.click(screen.getByRole("button", { name: "检查更新" }));

    expect(await screen.findByText("暂无公开发布版本。请稍后重试，或打开发布页查看。")).toBeInTheDocument();
    expect(screen.queryByText(/404/)).not.toBeInTheDocument();
  });
});

describe("model selection and provider model management", () => {
  it("moves a newly selected model to the recent tail", async () => {
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
      "+qwen3:4b-fast",
      "✓gemma4:latest",
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
    render(<ErrorToastProvider><ProviderModelManager provider={provider} serveRunning onSaved={onSaved} /></ErrorToastProvider>);
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
    expect(within(screen.getByTestId("error-toast-viewport")).getByText("已保存 2 个模型"))
      .toBeInTheDocument();
    expect(document.querySelector(".provider-model-manager .banner.ok")).toBeNull();
    await user.click(screen.getByRole("button", { name: "刷新模型" }));
    expect(await screen.findByText("暂时无法获取最新的供应商数据。请保留当前设置，稍后再次刷新。")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "保存模型" }));
    expect(await screen.findByText("操作未能完成。请重试；如果仍然失败，请从自救模式打开本地日志。")).toBeInTheDocument();
    expect(within(screen.getByText(/代理运行中/).parentElement!).getByRole("button")).toBeEnabled();
  });

  it("uses the shared Select for the provider credential source", async () => {
    const user = userEvent.setup();
    const provider: ProviderView = {
      name: "openai", provider: "openai", base_url: "https://api.example/v1",
      models: ["gpt-test"], has_auth: true,
    };

    const { container } = render(
      <ProviderModelManager provider={provider} serveRunning={false} onSaved={vi.fn()} />,
    );

    expect(container.querySelector('select[aria-label="编辑凭据来源"]')).toBeNull();
    const trigger = screen.getByRole("combobox", { name: "编辑凭据来源" });
    expect(trigger).toHaveAttribute("data-slot", "select-trigger");
    await user.click(trigger);
    expect(screen.getByRole("option", { name: "本地存储（默认）" })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "环境变量" })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "凭据文件" })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "无鉴权" })).toBeInTheDocument();
  });

  it("keeps Azure deployment configuration manual without invoking model discovery", async () => {
    const provider: ProviderView = {
      name: "azure",
      provider: "azure-openai-v1",
      base_url: "https://fixture.openai.azure.com/openai/v1",
      models: ["deployment-fixture"],
      has_auth: true,
    };
    const user = userEvent.setup();
    render(<ProviderModelManager provider={provider} serveRunning={false} onSaved={vi.fn()} />);

    await user.click(screen.getByRole("button", { name: "刷新模型" }));

    expect(discoverProviderModels).not.toHaveBeenCalled();
    expect(screen.getByText(/Azure deployment name 需要手工填写/)).toBeInTheDocument();
  });

  it("reports South as the active transport for a provider that names no engine", async () => {
    window.localStorage.setItem("token-station-language", "en");
    const provider: ProviderView = {
      name: "openai",
      provider: "openai-compatible",
      base_url: "https://api.example/v1",
      models: ["gpt-test"],
      has_auth: true,
      credential_source: "store",
      south_v1_available: true,
      south_v1_unavailable_reason: null,
      south_header_auth_v1_available: true,
      south_header_auth_v1_unavailable_reason: null,
    };
    vi.mocked(editProvider).mockResolvedValue(state);
    const user = userEvent.setup();

    render(
      <ErrorToastProvider>
        <ProviderModelManager provider={provider} serveRunning onSaved={vi.fn()} />
      </ErrorToastProvider>,
    );

    expect(screen.getByRole("status")).toHaveTextContent("South active");
    expect(screen.queryByRole("radio")).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Save details" }));
    await waitFor(() => expect(editProvider).toHaveBeenCalledWith(
      "openai",
      "https://api.example/v1",
      null,
      "store",
      null,
    ));
  });

  it("labels a provider pinned to Legacy and says how to return it to South", () => {
    window.localStorage.setItem("token-station-language", "en");
    const provider: ProviderView = {
      name: "openai",
      provider: "openai-compatible",
      base_url: "https://api.example/v1",
      models: ["gpt-test"],
      has_auth: true,
      credential_source: "store",
      provider_call: "legacy",
      south_v1_available: true,
      south_v1_unavailable_reason: null,
    };

    render(<ProviderModelManager provider={provider} serveRunning={false} onSaved={vi.fn()} />);

    const status = screen.getByRole("status");
    expect(status).toHaveTextContent("Legacy (pinned in configuration)");
    expect(status).toHaveTextContent(/Remove `provider_call`/);
  });

  it("reports a Legacy fallback with the host's reason when South cannot carry the provider", () => {
    window.localStorage.setItem("token-station-language", "en");
    const provider: ProviderView = {
      name: "local",
      provider: "openai-compatible",
      base_url: "http://127.0.0.1:11434/v1",
      models: ["local"],
      has_auth: false,
      credential_source: "none",
      south_v1_available: false,
      south_v1_unavailable_reason: "auth",
      south_header_auth_v1_available: false,
      south_header_auth_v1_unavailable_reason: "auth",
    };

    render(<ProviderModelManager provider={provider} serveRunning={false} onSaved={vi.fn()} />);

    const status = screen.getByRole("status");
    expect(status).toHaveTextContent("Legacy fallback");
    expect(status).toHaveTextContent(/requires credentials from the local store or an environment variable/);
  });

  it("keeps provider endpoint resolution failures accessibly linked for provider names with spaces", async () => {
    const provider: ProviderView = {
      name: "深度 seek 供应商", provider: "openai", base_url: "https://api.example/v1",
      models: ["gpt-test"], has_auth: true,
    };
    vi.mocked(previewProviderEndpoints).mockRejectedValueOnce(new Error("invalid endpoint"));

    render(
      <ErrorToastProvider>
        <ProviderModelManager provider={provider} serveRunning={false} onSaved={vi.fn()} />
      </ErrorToastProvider>,
    );

    const baseUrl = screen.getByRole("textbox", { name: "编辑 Base URL" });
    const endpointAlert = await screen.findByRole("alert");
    expect(endpointAlert).toHaveTextContent("操作未能完成");
    expect(endpointAlert.id).not.toMatch(/\s/);
    expect(baseUrl).toHaveAttribute("aria-describedby", endpointAlert.id);
    expect(baseUrl).toHaveAttribute("aria-invalid", "true");
    expect(baseUrl).toHaveAccessibleDescription(/操作未能完成/);
    expect(screen.getByRole("button", { name: "保存基本信息" })).toBeDisabled();
    expect(within(screen.getByTestId("error-toast-viewport")).queryByRole("alert")).toBeNull();
  });

  it("localizes a Chinese model-catalog warning in English mode", async () => {
    window.localStorage.setItem("token-station-language", "en");
    const provider: ProviderView = {
      name: "openai", provider: "openai", base_url: "https://api.example/v1",
      models: ["configured"], has_auth: true,
    };
    vi.mocked(discoverProviderModels).mockResolvedValue({
      models: ["configured"],
      source: "cache",
      fetched_at_ms: 1,
      warning: "模型目录请求失败：socket reset",
    });

    const user = userEvent.setup();
    render(<ProviderModelManager provider={provider} serveRunning={false} onSaved={vi.fn()} />);
    await user.click(screen.getByRole("button", { name: "Refresh models" }));

    expect(await screen.findByText(
      "The latest provider data is unavailable. Keep the current settings and try refreshing again later.",
    )).toBeInTheDocument();
    expect(screen.queryByText(/模型目录请求失败/)).not.toBeInTheDocument();
  });

  it("目录无权访问时明确显示内置预设而不是实时同步", async () => {
    const provider: ProviderView = {
      name: "kimi", provider: "openai-compatible", base_url: "https://api.moonshot.cn/v1",
      models: ["kimi-k3"], has_auth: true,
      model_capabilities: [{
        model: "kimi-k3", tool: "declared", vision: "unknown", json_schema: "declared",
        context_window: 1048576, max_output_tokens: 131072,
        context_window_source: "builtin_preset", max_output_tokens_source: "builtin_preset",
      }],
    };
    vi.mocked(discoverProviderModels).mockResolvedValue({
      models: [], source: "preset", fetched_at_ms: null,
      warning: "Key 无效，或当前账号没有读取模型目录的权限",
      capabilities_updated: true,
    });

    const user = userEvent.setup();
    render(<ProviderModelManager provider={provider} serveRunning={false} onSaved={vi.fn()} />);
    await user.click(screen.getByRole("button", { name: "刷新模型" }));

    expect(await screen.findByText("使用内置预设")).toBeInTheDocument();
    expect(screen.queryByText(/已同步/)).not.toBeInTheDocument();
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

  it("allows a user to complete missing model limits and rejects output above context", async () => {
    const provider: ProviderView = {
      name: "kimi",
      provider: "openai-compatible",
      base_url: "https://api.moonshot.cn/v1",
      models: ["kimi-k3"],
      model_capabilities: [{
        model: "kimi-k3",
        tool: "declared",
        vision: "unknown",
        json_schema: "declared",
        context_window: 1048576,
        max_output_tokens: 131072,
        context_window_source: "builtin_preset",
        max_output_tokens_source: "builtin_preset",
      }],
      has_auth: true,
    };
    vi.mocked(setProviderModelLimits).mockResolvedValue(state);
    const onSaved = vi.fn();
    const user = userEvent.setup();

    render(
      <ErrorToastProvider>
        <ProviderModelManager provider={provider} serveRunning onSaved={onSaved} />
      </ErrorToastProvider>,
    );

    expect(screen.getByText("限制来源 · 内置预设（非实时值）")).toBeInTheDocument();
    expect(screen.queryByText("该模型元数据缺少最大输出上限")).not.toBeInTheDocument();
    const output = screen.getByRole("spinbutton", { name: "kimi-k3 最大输出 Token" });
    await user.clear(output);
    await user.type(output, "1048577");
    await user.click(screen.getByRole("button", { name: "保存 kimi-k3 模型限制" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("最大输出 Token 不能大于上下文上限");
    expect(setProviderModelLimits).not.toHaveBeenCalled();

    await user.clear(output);
    await user.type(output, "32768");
    await user.click(screen.getByRole("button", { name: "保存 kimi-k3 模型限制" }));
    await waitFor(() => expect(setProviderModelLimits).toHaveBeenCalledWith(
      "kimi",
      "kimi-k3",
      1048576,
      32768,
    ));
    expect(onSaved).toHaveBeenCalledWith(state);
    expect(within(screen.getByTestId("error-toast-viewport")).getByText(
      "已保存模型限制；重启代理后生效",
    )).toBeInTheDocument();
  });

  it("目录中未配置的模型不提供必然失败的限制保存入口", () => {
    const provider: ProviderView = {
      name: "catalog-only", provider: "openai-compatible", base_url: "https://api.example/v1",
      models: ["configured"], has_auth: true,
      catalog: [{ model: "catalog-only-model", tool: "unknown", vision: "unknown", json_schema: "unknown", source: "live", last_seen_ms: 1, catalog_state: "active" }],
    };

    render(<ProviderModelManager provider={provider} serveRunning={false} onSaved={vi.fn()} />);

    expect(screen.queryByRole("button", { name: "保存 catalog-only-model 模型限制" })).not.toBeInTheDocument();
    expect(screen.queryByText(/catalog-only-model.*OpenCode/)).not.toBeInTheDocument();
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
    expect(screen.getByLabelText("供应商健康状态：健康")).toBeInTheDocument();
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
    expect(await screen.findByLabelText("供应商健康状态：不可用")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "运行分层测试" }));
    expect(await screen.findByLabelText("供应商健康状态：能力退化")).toBeInTheDocument();
  });
});

describe("provider deletion lifecycle", () => {
  it("供应商管理列表复用稳定品牌图标", () => {
    render(
      <ProviderList
        providers={[{
          name: "team-openai",
          brand_id: "openai",
          provider: "openai",
          base_url: "https://api.openai.com/v1",
          models: ["gpt-5.6"],
          has_auth: true,
        }]}
        deletedProviders={[]}
        recoveryError={null}
        serveRunning={false}
        busy={false}
        onRemove={vi.fn()}
        onRestore={vi.fn()}
        onStateChange={vi.fn()}
      />,
    );

    expect(document.querySelector('[data-provider-brand="openai"]')).toBeInTheDocument();
    expect(document.querySelector(".provider-monogram")?.getAttribute("aria-hidden")).toBe("true");
    const providerGroup = screen.getByRole("group", { name: "team-openai 供应商" });
    expect(within(providerGroup).getByText("team-openai", { selector: ".provider-identity-name" }))
      .toBeInTheDocument();
    expect(within(providerGroup).getByText("1 个模型")).toBeInTheDocument();
    const modelList = screen.getByRole("list", { name: "team-openai 模型" });
    expect(modelList).toHaveAttribute("data-layout", "compact-three-column");
    expect(within(modelList).getByRole("listitem"))
      .toHaveTextContent("gpt-5.6供应商 · team-openai");
  });

  it("供应商管理不根据自定义 deepseek 名称伪造官方品牌", () => {
    render(
      <ProviderList
        providers={[{
          name: "deepseek",
          brand_id: null,
          provider: "openai-compatible",
          base_url: "https://custom.example/v1",
          models: ["custom-model"],
          has_auth: true,
        }]}
        deletedProviders={[]}
        recoveryError={null}
        serveRunning={false}
        busy={false}
        onRemove={vi.fn()}
        onRestore={vi.fn()}
        onStateChange={vi.fn()}
      />,
    );

    const providerCard = screen.getByRole("list", { name: "deepseek 模型" }).closest(".provider-card");
    if (!providerCard) throw new Error("custom provider card missing");
    expect(providerCard.querySelector('[data-provider-brand="deepseek"]')).toBeNull();
    expect(within(providerCard as HTMLElement).getByText("D", { selector: ".brand-fallback" }))
      .toBeInTheDocument();
    expect(providerCard.querySelector('[data-provider-artwork="fallback"]')).toBeInTheDocument();
  });

  it("does not expose a raw provider recovery error in English mode", () => {
    window.localStorage.setItem("token-station-language", "en");
    render(
      <ProviderList
        providers={[]}
        deletedProviders={[]}
        recoveryError={"恢复供应商失败：/Users/example/private.json"}
        serveRunning={false}
        busy={false}
        onRemove={vi.fn()}
        onRestore={vi.fn()}
        onStateChange={vi.fn()}
      />,
    );

    expect(screen.getByText(
      "The operation could not be completed. Try again. If it still fails, open the local logs from Recovery mode.",
    )).toBeInTheDocument();
    expect(screen.queryByText(/\/Users\/example/)).not.toBeInTheDocument();
  });

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
