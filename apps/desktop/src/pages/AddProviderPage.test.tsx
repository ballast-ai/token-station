import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import type { FreeProviderPresetView } from "../api";
import AddProviderPage, {
  type FreeCatalogFilters,
  type ProviderCatalogMode,
  type RegularCatalogFilters,
} from "./AddProviderPage";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async (command: string, args?: { baseUrl?: string }) => {
    if (command === "preview_provider_endpoints") {
      const loopback = args?.baseUrl?.startsWith("http://127.0.0.1")
        || args?.baseUrl?.startsWith("http://localhost")
        || false;
      return {
        chat: "https://api.minimaxi.com/v1/chat/completions",
        responses: "https://api.minimaxi.com/v1/responses",
        messages: "https://api.minimaxi.com/v1/messages",
        loopback,
      };
    }
    if (command === "add_provider_with_credential") return {};
    throw new Error(`unexpected IPC command: ${command}`);
  }),
}));

// The provider picker is a clickable brand-card catalog, not a <select>; click cards by visible label.
const pickPreset = (user: ReturnType<typeof userEvent.setup>, label: string) =>
  user.click(screen.getByText(label, { selector: ".provider-catalog-card-title strong" }));

const regularFilters: RegularCatalogFilters = { query: "", region: "all" };
const freeFilters: FreeCatalogFilters = { query: "", offer: "all", region: "all" };
const freePresets: FreeProviderPresetView[] = [
  {
    id: "siliconflow",
    upstream_name: "siliconflow_free",
    label: "硅基流动 SiliconFlow",
    short_label: "SF",
    base_url: "https://api.siliconflow.cn/v1",
    offer_kind: "recurring",
    region: "china",
    tags: ["长期免费", "中国可用", "开源模型"],
    free_note: "仅展示官方免费模型",
    key_instruction: "创建 Key",
    application_url: "https://example.com/sf",
    docs_url: "https://example.com/sf/docs",
    verified_at: "2026-07-27",
    overage_policy: "rate_limited",
    models: [{
      id: "deepseek-ai/DeepSeek-V3.2",
      label: "DeepSeek V3.2",
      tool: "declared",
      vision: "unknown",
      json_schema: "declared",
      context_window: 128000,
    }],
  },
  {
    id: "cohere",
    upstream_name: "cohere_free",
    label: "Cohere",
    short_label: "CO",
    base_url: "https://api.cohere.ai/compatibility/v1",
    offer_kind: "trial",
    region: "global",
    tags: ["试用额度", "全球平台", "Trial Key"],
    free_note: "免费 Trial Key",
    key_instruction: "复制 Trial Key",
    application_url: "https://example.com/cohere",
    docs_url: "https://example.com/cohere/docs",
    verified_at: "2026-07-27",
    overage_policy: "hard_stop",
    models: [{
      id: "command-r7b",
      label: "Command R7B",
      tool: "declared",
      vision: "unknown",
      json_schema: "declared",
      context_window: 128000,
    }],
  },
  {
    id: "nvidia",
    upstream_name: "nvidia_free",
    label: "NVIDIA API Catalog",
    short_label: "NV",
    base_url: "https://integrate.api.nvidia.com/v1",
    offer_kind: "recurring",
    region: "global",
    tags: ["长期免费", "全球平台", "开发用途"],
    free_note: "build.nvidia.com 托管 API",
    key_instruction: "点击 Get API Key",
    application_url: "https://build.nvidia.com",
    docs_url: "https://example.com/nvidia/docs",
    verified_at: "2026-07-27",
    overage_policy: "rate_limited",
    models: [{
      id: "openai/gpt-oss-120b",
      label: "GPT-OSS 120B",
      tool: "declared",
      vision: "unknown",
      json_schema: "declared",
      context_window: 131072,
    }],
  },
];

function renderPage(overrides: {
  existingNames?: string[];
  catalogMode?: ProviderCatalogMode;
  entryMode?: "provider-first" | "model-first";
} = {}) {
  return render(
    <AddProviderPage
      existingNames={overrides.existingNames ?? []}
      onCancel={vi.fn()}
      onAdded={vi.fn()}
      catalogMode={overrides.catalogMode ?? "regular"}
      onCatalogModeChange={vi.fn()}
      regularFilters={regularFilters}
      onRegularFiltersChange={vi.fn()}
      freePresets={[]}
      freeLoading={false}
      freeError=""
      freeFilters={freeFilters}
      onFreeFiltersChange={vi.fn()}
      onLoadFree={vi.fn()}
      onSelectFree={vi.fn()}
      entryMode={overrides.entryMode}
    />,
  );
}

function FreeCatalogHarness() {
  const [filters, setFilters] = useState<FreeCatalogFilters>(freeFilters);
  return (
    <AddProviderPage
      existingNames={[]}
      onCancel={vi.fn()}
      onAdded={vi.fn()}
      catalogMode="free"
      onCatalogModeChange={vi.fn()}
      regularFilters={regularFilters}
      onRegularFiltersChange={vi.fn()}
      freePresets={freePresets}
      freeLoading={false}
      freeError=""
      freeFilters={filters}
      onFreeFiltersChange={setFilters}
      onLoadFree={vi.fn()}
      onSelectFree={vi.fn()}
    />
  );
}

function RegularCatalogHarness() {
  const [filters, setFilters] = useState<RegularCatalogFilters>(regularFilters);
  return (
    <AddProviderPage
      existingNames={[]}
      onCancel={vi.fn()}
      onAdded={vi.fn()}
      catalogMode="regular"
      onCatalogModeChange={vi.fn()}
      regularFilters={filters}
      onRegularFiltersChange={setFilters}
      freePresets={[]}
      freeLoading={false}
      freeError=""
      freeFilters={freeFilters}
      onFreeFiltersChange={vi.fn()}
      onLoadFree={vi.fn()}
      onSelectFree={vi.fn()}
    />
  );
}

describe("AddProviderPage", () => {
  it("searches models first and then opens the selected provider with only that model selected", async () => {
    window.localStorage.setItem("token-station-language", "zh-CN");
    const user = userEvent.setup();
    renderPage({ entryMode: "model-first" });

    await user.type(screen.getByRole("searchbox", { name: "搜索模型" }), "gpt-5.6-sol");
    const results = screen.getByRole("list", { name: "模型与供应商" });
    const result = within(results).getByRole("button", { name: /gpt-5.6-sol.*OpenAI/ });
    expect(result.textContent?.indexOf("gpt-5.6-sol"))
      .toBeLessThan(result.textContent?.indexOf("OpenAI") ?? -1);

    await user.click(result);
    expect(screen.getByRole("heading", { name: "OpenAI" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /gpt-5.6-sol/ })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("button", { name: /gpt-5.6-terra/ })).toHaveAttribute("aria-pressed", "false");
  });

  it("filters standard providers by a model name across punctuation boundaries", async () => {
    window.localStorage.setItem("token-station-language", "en");
    const user = userEvent.setup();
    render(<RegularCatalogHarness />);

    await user.type(screen.getByRole("searchbox", { name: "Search standard providers" }), "gpt 5.6 sol");

    const providers = screen.getByRole("list", { name: "Standard providers" });
    expect(within(providers).getByText("OpenAI", { selector: "strong" })).toBeInTheDocument();
    expect(within(providers).queryByText("Anthropic Claude", { selector: "strong" })).not.toBeInTheDocument();
  });

  it("renders localized provider names in the default English interface", () => {
    window.localStorage.removeItem("token-station-language");
    renderPage();

    expect(screen.getByText("MiniMax (China)")).toBeInTheDocument();
    expect(screen.getByText("Local Ollama")).toBeInTheDocument();
    expect(screen.queryByText("MiniMax（中国）")).not.toBeInTheDocument();
    expect(screen.queryByText("本地 Ollama")).not.toBeInTheDocument();
  });

  it("switches the unified catalog to free APIs", async () => {
    const user = userEvent.setup();
    const onCatalogModeChange = vi.fn();
    const onLoadFree = vi.fn();
    render(
      <AddProviderPage
        existingNames={[]}
        onCancel={vi.fn()}
        onAdded={vi.fn()}
        catalogMode="regular"
        onCatalogModeChange={onCatalogModeChange}
        regularFilters={regularFilters}
        onRegularFiltersChange={vi.fn()}
        freePresets={[]}
        freeLoading={false}
        freeError=""
        freeFilters={freeFilters}
        onFreeFiltersChange={vi.fn()}
        onLoadFree={onLoadFree}
        onSelectFree={vi.fn()}
      />,
    );

    await user.click(screen.getByRole("button", { name: /免费 API/ }));
    expect(onCatalogModeChange).toHaveBeenCalledWith("free");
    expect(onLoadFree).toHaveBeenCalledOnce();
  });

  it("shows the endpoint and credential boundary for a catalog preset", async () => {
    const user = userEvent.setup();
    renderPage();

    await pickPreset(user, "MiniMax（中国）");

    expect(screen.getByDisplayValue("https://api.minimaxi.com/v1")).toBeDisabled();
    expect(screen.getByText("中国开放平台；与国际站 Key 不通用。")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "官方接入文档" })).toHaveAttribute(
      "href",
      "https://platform.minimaxi.com/docs/api-reference/text-openai-api",
    );
    expect(screen.getByText("MiniMax-M3")).toBeInTheDocument();
    expect(await screen.findByText("https://api.minimaxi.com/v1/chat/completions")).toBeInTheDocument();
    expect(screen.getByText("https://api.minimaxi.com/v1/responses")).toBeInTheDocument();
    expect(screen.getByText("https://api.minimaxi.com/v1/messages")).toBeInTheDocument();
  });

  it("disables the local-model flag for the DeepSeek cloud preset", async () => {
    window.localStorage.setItem("token-station-language", "zh-CN");
    const user = userEvent.setup();
    renderPage();

    await pickPreset(user, "DeepSeek");

    const local = screen.getByRole("checkbox", { name: /这是本机运行的本地模型/ });
    expect(local).not.toBeChecked();
    expect(local).toBeDisabled();
    expect(await screen.findByText(/云端地址不能标记为本地模型/)).toBeInTheDocument();
  });

  it("localizes a model-catalog warning returned while adding a provider", async () => {
    window.localStorage.setItem("token-station-language", "en");
    const user = userEvent.setup();
    renderPage();

    await pickPreset(user, "OpenAI");
    await screen.findByText("https://api.minimaxi.com/v1/chat/completions");
    await user.type(screen.getByLabelText("API Key"), "test-key");
    vi.mocked(invoke).mockResolvedValueOnce({
      models: ["gpt-test"],
      source: "cache",
      fetched_at_ms: 1,
      warning: "模型目录请求失败：socket reset",
    });

    await user.click(screen.getByRole("button", { name: "Refresh models" }));

    expect(await screen.findByText(
      "The latest provider data is unavailable. Keep the current settings and try refreshing again later.",
    )).toBeInTheDocument();
    expect(screen.queryByText(/模型目录请求失败/)).not.toBeInTheDocument();
  });

  it("keeps the local-model flag available for the Ollama loopback preset", async () => {
    window.localStorage.setItem("token-station-language", "zh-CN");
    const user = userEvent.setup();
    renderPage();

    await pickPreset(user, "本地 Ollama");

    const local = screen.getByRole("checkbox", { name: /这是本机运行的本地模型/ });
    expect(local).toBeChecked();
    expect(local).toBeEnabled();
    expect(await screen.findByText(/已检测到本机回环地址/)).toBeInTheDocument();
  });

  it("uses local store by default and submits env credentials as references only", async () => {
    window.localStorage.setItem("token-station-language", "zh-CN");
    const user = userEvent.setup();
    renderPage();
    await pickPreset(user, "DeepSeek");

    expect(screen.getByLabelText("API Key")).toBeInTheDocument();
    await user.click(screen.getByText("高级凭据来源"));
    await user.click(screen.getByRole("combobox", { name: "凭据来源" }));
    await user.click(await screen.findByRole("option", { name: "环境变量" }));
    expect(screen.queryByLabelText("API Key")).not.toBeInTheDocument();
    await user.type(screen.getByLabelText("环境变量名"), "DEEPSEEK_API_KEY");
    await user.click(screen.getByRole("button", { name: "添加供应商" }));

    expect(vi.mocked(invoke)).toHaveBeenCalledWith("add_provider_with_credential", expect.objectContaining({
      name: "deepseek",
      apiKey: null,
      local: false,
      credentialSource: "env",
      credentialReference: "DEEPSEEK_API_KEY",
    }));
  });

  it("offers a closed Azure OpenAI v1 dialect for custom providers", async () => {
    window.localStorage.setItem("token-station-language", "zh-CN");
    const user = userEvent.setup();
    renderPage();

    await pickPreset(user, "自定义配置");
    await user.click(screen.getByRole("combobox", { name: "API 方言" }));
    await user.click(await screen.findByRole("option", { name: "Azure OpenAI v1" }));

    expect(screen.getByText(/Base URL 必须指向资源的 \/openai\/v1 根路径/)).toBeInTheDocument();
    expect(screen.getByText(/模型名填写 Azure deployment name/)).toBeInTheDocument();

    await user.type(screen.getByLabelText("名称"), "azure");
    await user.type(
      screen.getByLabelText("Base URL"),
      "https://fixture.openai.azure.com/openai/v1",
    );
    await user.type(screen.getByLabelText("API Key"), "synthetic-key");
    await screen.findByText("https://api.minimaxi.com/v1/chat/completions");
    vi.mocked(invoke).mockClear();

    await user.click(screen.getByRole("button", { name: "刷新模型" }));

    expect(vi.mocked(invoke)).not.toHaveBeenCalledWith(
      "discover_provider_models",
      expect.anything(),
    );
    expect(screen.getByText(/Azure deployment name 需要手工填写/)).toBeInTheDocument();
  });

  it("drops discovered OpenAI models when a custom provider switches to Azure", async () => {
    window.localStorage.setItem("token-station-language", "zh-CN");
    const user = userEvent.setup();
    renderPage();

    await pickPreset(user, "自定义配置");
    await user.type(screen.getByLabelText("名称"), "azure");
    await user.type(screen.getByLabelText("Base URL"), "https://api.example/v1");
    await user.type(screen.getByLabelText("API Key"), "synthetic-key");
    await screen.findByText("https://api.minimaxi.com/v1/chat/completions");
    vi.mocked(invoke).mockResolvedValueOnce({
      models: ["openai-discovered-model"],
      source: "live",
      fetched_at_ms: 1,
      warning: null,
    });

    await user.click(screen.getByRole("button", { name: "刷新模型" }));
    const discovered = await screen.findByRole("button", { name: /openai-discovered-model/ });
    await user.click(discovered);
    expect(discovered).toHaveAttribute("aria-pressed", "true");

    await user.click(screen.getByRole("combobox", { name: "API 方言" }));
    await user.click(await screen.findByRole("option", { name: "Azure OpenAI v1" }));

    expect(screen.queryByRole("button", { name: /openai-discovered-model/ })).not.toBeInTheDocument();
  });

  it("凭据来源使用组件库下拉而非原生 select", async () => {
    window.localStorage.setItem("token-station-language", "zh-CN");
    const user = userEvent.setup();
    const { container } = renderPage();
    await pickPreset(user, "DeepSeek");
    await user.click(screen.getByText("高级凭据来源"));

    const trigger = screen.getByRole("combobox", { name: "凭据来源" });
    expect(container.querySelector('select[aria-label="凭据来源"]')).not.toBeInTheDocument();
    expect(trigger).toHaveAttribute("data-slot", "select-trigger");
  });

  it("offers official and self-hosted presets as cards and omits non-default aggregators", () => {
    renderPage();

    const grid = screen.getByRole("list", { name: "常规供应商列表" });
    // Representatives of official usage-based APIs and local self-hosted providers both appear as cards.
    expect(within(grid).getByText("MiniMax（中国）")).toBeInTheDocument();
    expect(within(grid).getByText("本地 Ollama")).toBeInTheDocument();
    // Aggregator candidates are excluded from the default catalog and do not appear as cards.
    expect(screen.queryByText(/OpenRouter/)).toBeNull();
  });

  it("keeps regular provider choices compact with only icon and name", () => {
    renderPage();

    const grid = screen.getByRole("list", { name: "常规供应商列表" });
    expect(within(grid).getByText("OpenAI", { selector: "strong" })).toBeInTheDocument();
    expect(within(grid).queryByText("openai", { selector: "code" })).toBeNull();
    expect(within(grid).queryByText("7 模型")).toBeNull();
    expect(within(grid).queryByText("全球开放平台 · 按量 API")).toBeNull();
  });

  it("frames an existing provider as a safe update", async () => {
    const user = userEvent.setup();
    renderPage({ existingNames: ["deepseek"] });

    await pickPreset(user, "DeepSeek");

    expect(screen.getByText(/已经存在/)).toBeInTheDocument();
    expect(screen.getByText(/不会重复创建/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "更新供应商" })).toBeInTheDocument();
  });

  it("searches free provider names, models, descriptions and tags", async () => {
    const user = userEvent.setup();
    render(<FreeCatalogHarness />);

    await user.type(screen.getByRole("searchbox", { name: "搜索免费供应商" }), "gpt oss 120b");
    const grid = screen.getByRole("list", { name: "免费供应商列表" });
    expect(within(grid).getByText("NVIDIA API Catalog")).toBeInTheDocument();
    expect(within(grid).queryByText("Cohere")).toBeNull();
  });

  it("keeps free provider choices compact while preserving the fee type", () => {
    render(<FreeCatalogHarness />);

    const grid = screen.getByRole("list", { name: "免费供应商列表" });
    expect(within(grid).getByText("NVIDIA API Catalog", { selector: "strong" })).toHaveAttribute(
      "title",
      "NVIDIA API Catalog",
    );
    expect(within(grid).getAllByText("长期免费").length).toBeGreaterThan(0);
    expect(within(grid).queryByText("nvidia_free", { selector: "code" })).toBeNull();
    expect(within(grid).queryByText("3 模型")).toBeNull();
    expect(within(grid).queryByText("build.nvidia.com 托管 API")).toBeNull();
    expect(within(grid).queryByText("全球平台")).toBeNull();
  });

  it("combines free offer and region filters and can clear an empty result", async () => {
    const user = userEvent.setup();
    render(<FreeCatalogHarness />);

    await user.click(screen.getByRole("button", { name: "试用额度" }));
    await user.click(screen.getByRole("button", { name: "中国可用" }));
    expect(screen.getByText("没有匹配的供应商")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "清除筛选" }));
    expect(screen.getByText("硅基流动 SiliconFlow")).toBeInTheDocument();
    expect(screen.getByText("Cohere", { selector: "strong" })).toBeInTheDocument();
  });
});
