import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import type { FreeProviderPresetView, StateView } from "../api";
import { PROVIDER_CATALOG } from "../catalog";
import { ErrorToastProvider } from "../components/ErrorToast";
import AddProviderPage, {
  type FreeCatalogFilters,
  type ProviderCatalogMode,
  type RegularCatalogFilters,
} from "./AddProviderPage";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

beforeEach(() => {
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "preview_provider_endpoints") {
      const baseUrl = (args as { baseUrl?: string } | undefined)?.baseUrl;
      const loopback = baseUrl?.startsWith("http://127.0.0.1")
        || baseUrl?.startsWith("http://localhost")
        || false;
      return {
        chat: "https://api.minimaxi.com/v1/chat/completions",
        responses: "https://api.minimaxi.com/v1/responses",
        messages: "https://api.minimaxi.com/v1/messages",
        loopback,
      };
    }
    if (command === "list_public_provider_models") {
      return {
        providers: {},
        source: "cache",
        fetched_at_ms: 42,
        unavailable_provider_ids: [],
      };
    }
    if (command === "add_provider_with_credential") return {};
    if (command === "discover_provider_models" || command === "discover_provider_model_limits") {
      return {
        models: [],
        source: "none",
        fetched_at_ms: null,
        warning: "model limits unavailable",
        capabilities_updated: false,
      };
    }
    if (command === "get_state") return { source: "fresh-state" };
    if (command === "import_model_prices_for_provider") {
      return {
        state: { source: "price-import" },
        imported: 2,
        existing: 0,
        missing_model_ids: [],
        price_version: 3,
      };
    }
    throw new Error(`unexpected IPC command: ${command}`);
  });
});

// The provider picker is a clickable brand-card catalog, not a <select>; click cards by visible label.
const pickPreset = (user: ReturnType<typeof userEvent.setup>, label: string) =>
  user.click(screen.getByText(label, { selector: ".provider-catalog-card-title strong" }));

const pickModel = (user: ReturnType<typeof userEvent.setup>, label: string) =>
  user.click(screen.getByRole("button", {
    name: (accessibleName) => accessibleName.endsWith(label),
  }));

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
  onAdded?: (state: StateView, message: string) => void;
  onStateChanged?: (state: StateView) => void;
} = {}) {
  return render(
    <ErrorToastProvider>
      <AddProviderPage
        existingNames={overrides.existingNames ?? []}
        onCancel={vi.fn()}
        onAdded={overrides.onAdded ?? vi.fn()}
        onStateChanged={overrides.onStateChanged}
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
      />
    </ErrorToastProvider>,
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
  it("refreshes model limits after creation without blocking on discovery failure", async () => {
    window.localStorage.setItem("token-station-language", "zh-CN");
    const user = userEvent.setup();
    const onAdded = vi.fn();
    const baseImplementation = vi.mocked(invoke).getMockImplementation();
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "discover_provider_model_limits") throw new Error("catalog unavailable");
      return baseImplementation?.(command, args);
    });
    renderPage({ onAdded });

    await pickPreset(user, "DeepSeek");
    await pickModel(user, "deepseek-v4-flash");
    await user.type(screen.getByLabelText("API Key"), "test-key");
    await user.click(screen.getByRole("button", { name: "添加供应商" }));

    await waitFor(() => expect(vi.mocked(invoke)).toHaveBeenCalledWith(
      "discover_provider_model_limits",
      { name: "deepseek", baseUrl: "https://api.deepseek.com/v1" },
    ));
    expect(onAdded).toHaveBeenCalledWith({}, expect.any(String));
  });

  it("publishes the added Provider before slow model discovery finishes", async () => {
    window.localStorage.setItem("token-station-language", "zh-CN");
    const user = userEvent.setup();
    const onAdded = vi.fn();
    let resolveDiscovery: ((value: unknown) => void) | undefined;
    const baseImplementation = vi.mocked(invoke).getMockImplementation();
    vi.mocked(invoke).mockImplementation((command, args) => {
      if (command === "discover_provider_model_limits") {
        return new Promise((resolve) => {
          resolveDiscovery = resolve;
        });
      }
      return baseImplementation?.(command, args) as Promise<unknown>;
    });
    renderPage({ onAdded });

    await pickPreset(user, "DeepSeek");
    await pickModel(user, "deepseek-v4-flash");
    await user.type(screen.getByLabelText("API Key"), "test-key");
    await user.click(screen.getByRole("button", { name: "添加供应商" }));

    await waitFor(() => expect(onAdded).toHaveBeenCalledOnce());
    resolveDiscovery?.({
      models: [],
      source: "none",
      fetched_at_ms: null,
      warning: "timed out",
      capabilities_updated: false,
    });
  });

  it("publishes refreshed Provider state when live discovery updates model limits", async () => {
    window.localStorage.setItem("token-station-language", "zh-CN");
    const user = userEvent.setup();
    const onAdded = vi.fn();
    const onStateChanged = vi.fn();
    const baseImplementation = vi.mocked(invoke).getMockImplementation();
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "discover_provider_model_limits") {
        return {
          models: ["deepseek-v4-flash"],
          source: "live",
          fetched_at_ms: 42,
          warning: null,
          capabilities_updated: true,
        };
      }
      return baseImplementation?.(command, args);
    });
    renderPage({ onAdded, onStateChanged });

    await pickPreset(user, "DeepSeek");
    await pickModel(user, "deepseek-v4-flash");
    await user.type(screen.getByLabelText("API Key"), "test-key");
    await user.click(screen.getByRole("button", { name: "添加供应商" }));

    expect(onAdded).toHaveBeenCalledWith({}, expect.any(String));
    await waitFor(() => expect(onStateChanged).toHaveBeenCalledWith(
      expect.objectContaining({ source: "fresh-state" }),
    ));
  });

  it("binds Add Provider to the exact live catalog revision already shown", async () => {
    window.localStorage.setItem("token-station-language", "zh-CN");
    const user = userEvent.setup();
    const baseImplementation = vi.mocked(invoke).getMockImplementation();
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "discover_provider_models") {
        return {
          models: ["deepseek-v4-flash"],
          source: "live",
          fetched_at_ms: 42,
          warning: null,
          capabilities_updated: false,
          revision: 7,
          catalog: [],
          added: [],
          removed: [],
        };
      }
      if (command === "add_provider_with_credential") {
        return {
          providers: [{
            name: "deepseek",
            model_capabilities: [{
              model: "deepseek-v4-flash",
              context_window_source: "provider",
            }],
          }],
        };
      }
      return baseImplementation?.(command, args);
    });
    renderPage();

    await pickPreset(user, "DeepSeek");
    await pickModel(user, "deepseek-v4-flash");
    await user.type(screen.getByLabelText("API Key"), "test-key");
    await user.click(screen.getByRole("button", { name: "刷新模型" }));
    await screen.findByText("已同步 1 个");
    await user.click(screen.getByRole("button", { name: "添加供应商" }));

    expect(vi.mocked(invoke)).toHaveBeenCalledWith(
      "add_provider_with_credential",
      expect.objectContaining({ catalogRevision: 7 }),
    );
    expect(vi.mocked(invoke)).not.toHaveBeenCalledWith(
      "discover_provider_model_limits",
      expect.anything(),
    );
  });

  it("retries limit discovery when Add Provider cannot consume the shown revision", async () => {
    window.localStorage.setItem("token-station-language", "zh-CN");
    const user = userEvent.setup();
    const baseImplementation = vi.mocked(invoke).getMockImplementation();
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "discover_provider_models") {
        return {
          models: ["deepseek-v4-flash"],
          source: "live",
          fetched_at_ms: 42,
          warning: null,
          capabilities_updated: false,
          revision: 7,
        };
      }
      return baseImplementation?.(command, args);
    });
    renderPage();

    await pickPreset(user, "DeepSeek");
    await pickModel(user, "deepseek-v4-flash");
    await user.type(screen.getByLabelText("API Key"), "test-key");
    await user.click(screen.getByRole("button", { name: "刷新模型" }));
    await screen.findByText("已同步 1 个");
    await user.click(screen.getByRole("button", { name: "添加供应商" }));

    await waitFor(() => expect(vi.mocked(invoke)).toHaveBeenCalledWith(
      "discover_provider_model_limits",
      { name: "deepseek", baseUrl: "https://api.deepseek.com/v1" },
    ));
  });

  it("invalidates live catalog evidence when the API key changes", async () => {
    window.localStorage.setItem("token-station-language", "zh-CN");
    const user = userEvent.setup();
    const baseImplementation = vi.mocked(invoke).getMockImplementation();
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "discover_provider_models") {
        return {
          models: ["deepseek-v4-flash"],
          source: "live",
          fetched_at_ms: 42,
          warning: null,
          capabilities_updated: false,
          revision: 7,
        };
      }
      return baseImplementation?.(command, args);
    });
    renderPage();

    await pickPreset(user, "DeepSeek");
    await pickModel(user, "deepseek-v4-flash");
    const keyInput = screen.getByLabelText("API Key");
    await user.type(keyInput, "key-a");
    await user.click(screen.getByRole("button", { name: "刷新模型" }));
    await screen.findByText("已同步 1 个");
    await user.clear(keyInput);
    await user.type(keyInput, "key-b");
    await user.click(screen.getByRole("button", { name: "添加供应商" }));

    expect(vi.mocked(invoke)).toHaveBeenCalledWith(
      "add_provider_with_credential",
      expect.objectContaining({ apiKey: "key-b", catalogRevision: null }),
    );
  });

  it("imports all selected public prices in one batch when adding a cloud preset", async () => {
    window.localStorage.setItem("token-station-language", "zh-CN");
    const user = userEvent.setup();
    const onAdded = vi.fn();
    const onStateChanged = vi.fn();
    renderPage({ onAdded, onStateChanged });

    await pickPreset(user, "DeepSeek");
    expect(screen.getByRole("group", { name: "供应商凭据" }).closest(".provider-wizard"))
      .not.toHaveClass("panel");
    await pickModel(user, "deepseek-v4-flash");
    await pickModel(user, "deepseek-v4-pro");
    expect(screen.getByRole("checkbox", { name: "批量填充匹配的公开价格" })).toBeChecked();
    expect(screen.queryByRole("link", { name: "官方接入文档" })).not.toBeInTheDocument();
    expect(screen.queryByText(/使用 models\.dev 的公开美元标价作为估算/)).not.toBeInTheDocument();
    await user.type(screen.getByLabelText("API Key"), "test-key");
    await user.click(screen.getByRole("button", { name: "添加供应商" }));

    expect(vi.mocked(invoke)).toHaveBeenCalledWith("import_model_prices_for_provider", {
      upstreamName: "deepseek",
      modelIds: ["deepseek-v4-flash", "deepseek-v4-pro"],
    });
    expect(onAdded).toHaveBeenCalledWith(
      {},
      expect.any(String),
    );
    expect(onStateChanged).toHaveBeenCalledWith(
      expect.objectContaining({ source: "price-import" }),
    );
  });

  it("publishes the added Provider before a slow public price import finishes", async () => {
    window.localStorage.setItem("token-station-language", "zh-CN");
    const user = userEvent.setup();
    const onAdded = vi.fn();
    const onStateChanged = vi.fn();
    let resolveImport: ((value: unknown) => void) | undefined;
    const baseImplementation = vi.mocked(invoke).getMockImplementation();
    vi.mocked(invoke).mockImplementation((command, args) => {
      if (command === "import_model_prices_for_provider") {
        return new Promise((resolve) => {
          resolveImport = resolve;
        });
      }
      return baseImplementation?.(command, args) as Promise<unknown>;
    });
    renderPage({ onAdded, onStateChanged });

    await pickPreset(user, "DeepSeek");
    await pickModel(user, "deepseek-v4-flash");
    await user.type(screen.getByLabelText("API Key"), "test-key");
    await user.click(screen.getByRole("button", { name: "添加供应商" }));

    await waitFor(() => expect(onAdded).toHaveBeenCalledOnce());
    expect(onStateChanged).not.toHaveBeenCalled();

    resolveImport?.({
      state: { source: "price-import" },
      imported: 2,
      existing: 0,
      missing_model_ids: [],
      price_version: 1,
    });
    await waitFor(() => expect(onStateChanged).toHaveBeenCalledWith(
      expect.objectContaining({ source: "price-import" }),
    ));
  });

  it("keeps automatic public pricing off for custom providers", async () => {
    window.localStorage.setItem("token-station-language", "zh-CN");
    const user = userEvent.setup();
    renderPage();

    await pickPreset(user, "自定义配置");

    expect(screen.getByRole("checkbox", { name: "批量填充匹配的公开价格" }))
      .not.toBeChecked();
  });

  it("does not apply usage-list pricing to subscription or local presets", async () => {
    window.localStorage.setItem("token-station-language", "zh-CN");
    const user = userEvent.setup();
    renderPage();

    await pickPreset(user, "智谱 GLM（Coding Plan）");
    expect(screen.getByRole("checkbox", { name: "批量填充匹配的公开价格" }))
      .not.toBeChecked();

    await user.click(screen.getByRole("button", { name: "返回目录" }));
    await pickPreset(user, "本地 Ollama");
    expect(screen.getByRole("checkbox", { name: "批量填充匹配的公开价格" }))
      .toBeDisabled();
  });

  it("uses the regional public-catalog namespace for a preset", async () => {
    window.localStorage.setItem("token-station-language", "zh-CN");
    const user = userEvent.setup();
    renderPage();

    await pickPreset(user, "智谱 GLM（中国）");
    await pickModel(user, "glm-5.2");
    await user.type(screen.getByLabelText("API Key"), "test-key");
    await user.click(screen.getByRole("button", { name: "添加供应商" }));

    expect(vi.mocked(invoke)).toHaveBeenCalledWith("import_model_prices_for_provider", {
      upstreamName: "glm_cn",
      modelIds: ["glm-5.2"],
    });
  });

  it("keeps the added provider and reports a batch price import failure", async () => {
    window.localStorage.setItem("token-station-language", "zh-CN");
    const user = userEvent.setup();
    const onAdded = vi.fn();
    const onStateChanged = vi.fn();
    const baseImplementation = vi.mocked(invoke).getMockImplementation();
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "import_model_prices_for_provider") {
        throw new Error("catalog unavailable");
      }
      return baseImplementation?.(command, args);
    });
    renderPage({ onAdded, onStateChanged });

    await pickPreset(user, "DeepSeek");
    await pickModel(user, "deepseek-v4-flash");
    await user.type(screen.getByLabelText("API Key"), "test-key");
    await user.click(screen.getByRole("button", { name: "添加供应商" }));

    expect(onAdded).toHaveBeenCalledWith({}, expect.any(String));
    expect(onStateChanged).toHaveBeenCalledWith(
      expect.objectContaining({ source: "fresh-state" }),
    );
    expect(screen.getByRole("alert")).toHaveTextContent(
      "供应商已添加，但公开价格导入失败",
    );
  });

  it("searches models first and then opens the selected provider with only that model selected", async () => {
    window.localStorage.setItem("token-station-language", "zh-CN");
    const user = userEvent.setup();
    renderPage({ entryMode: "model-first" });

    await user.type(screen.getByRole("searchbox", { name: "搜索模型" }), "gpt-5.6-sol");
    const results = screen.getByRole("list", { name: "模型与供应商" });
    expect(results).toHaveAttribute("data-layout", "compact-three-column");
    const result = within(results).getByRole("button", { name: /gpt-5.6-sol.*OpenAI/ });
    expect(result.querySelector('[data-provider-brand="openai"]'))
      .toHaveStyle({ width: "22px", height: "22px" });
    expect(result.textContent?.indexOf("gpt-5.6-sol"))
      .toBeLessThan(result.textContent?.indexOf("OpenAI") ?? -1);

    await user.click(result);
    expect(screen.getByRole("heading", { name: "OpenAI" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /gpt-5.6-sol/ })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("button", { name: /gpt-5.6-terra/ })).toHaveAttribute("aria-pressed", "false");
  });

  it("searches one canonical GLM model across official and managed provider channels", async () => {
    window.localStorage.setItem("token-station-language", "zh-CN");
    const user = userEvent.setup();
    renderPage({ entryMode: "model-first" });

    await user.type(screen.getByRole("searchbox", { name: "搜索模型" }), "glm-5.2");
    const results = screen.getByRole("list", { name: "模型与供应商" });
    const alibaba = within(results).getByRole("button", {
      name: /glm-5\.2.*阿里云百炼（中国）/,
    });

    expect(alibaba).toHaveTextContent("glm-5.2");
    expect(alibaba).toHaveTextContent("ZHIPU/GLM-5.2");
    expect(alibaba).toHaveTextContent("托管推理");
    expect(within(results).getByRole("button", {
      name: /glm-5\.2.*硅基流动 SiliconFlow（中国）/,
    })).toBeInTheDocument();

    await user.click(alibaba);
    expect(screen.getByRole("heading", { name: "阿里云百炼（中国）" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /ZHIPU\/GLM-5\.2/ }))
      .toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("button", { name: /qwen3\.7-max/ }))
      .toHaveAttribute("aria-pressed", "false");
  });

  it("uses the refreshed public catalog for model-first search", async () => {
    window.localStorage.setItem("token-station-language", "zh-CN");
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "list_public_provider_models") {
        return {
          providers: {
            openai: ["gpt-current"],
            qwen: ["glm-5.2", "qwen-current"],
          },
          source: "live",
          fetched_at_ms: 42,
          unavailable_provider_ids: ["volcengine_ark"],
        };
      }
      if (command === "preview_provider_endpoints") {
        const baseUrl = (args as { baseUrl?: string } | undefined)?.baseUrl;
        return {
          chat: `${baseUrl}/chat/completions`,
          responses: `${baseUrl}/responses`,
          messages: `${baseUrl}/messages`,
          loopback: false,
        };
      }
      throw new Error(`unexpected IPC command: ${command}`);
    });

    const user = userEvent.setup();
    renderPage({ entryMode: "model-first" });

    expect(await screen.findByText(
      `公共目录已同步 · 2/${PROVIDER_CATALOG.length} 个渠道`,
    )).toBeInTheDocument();
    await user.type(screen.getByRole("searchbox", { name: "搜索模型" }), "gpt");
    const results = screen.getByRole("list", { name: "模型与供应商" });
    expect(within(results).getByRole("button", { name: /gpt-current.*OpenAI/ }))
      .toBeInTheDocument();
    expect(within(results).queryByRole("button", { name: /gpt-5\.6-sol.*OpenAI/ }))
      .not.toBeInTheDocument();
  });

  it("uses the refreshed public catalog when the model-first query is empty", async () => {
    window.localStorage.setItem("token-station-language", "zh-CN");
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "list_public_provider_models") {
        return {
          providers: { openai: ["gpt-current"] },
          source: "live",
          fetched_at_ms: 42,
          unavailable_provider_ids: [],
        };
      }
      if (command === "preview_provider_endpoints") {
        const baseUrl = (args as { baseUrl?: string } | undefined)?.baseUrl;
        return {
          chat: `${baseUrl}/chat/completions`,
          responses: `${baseUrl}/responses`,
          messages: `${baseUrl}/messages`,
          loopback: false,
        };
      }
      throw new Error(`unexpected IPC command: ${command}`);
    });

    renderPage({ entryMode: "model-first" });

    expect(await screen.findByText(
      `公共目录已同步 · 1/${PROVIDER_CATALOG.length} 个渠道`,
    )).toBeInTheDocument();
    const results = screen.getByRole("list", { name: "模型与供应商" });
    expect(within(results).getByRole("button", { name: /gpt-current.*OpenAI/ }))
      .toBeInTheDocument();
    expect(within(results).queryByRole("button", { name: /gpt-5\.6-sol.*OpenAI/ }))
      .not.toBeInTheDocument();
  });

  it("clears a hidden stale selection after a late public catalog refresh", async () => {
    window.localStorage.setItem("token-station-language", "zh-CN");
    let resolveCatalog!: (value: unknown) => void;
    const catalog = new Promise((resolve) => {
      resolveCatalog = resolve;
    });
    const baseImplementation = vi.mocked(invoke).getMockImplementation();
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "list_public_provider_models") return catalog;
      return baseImplementation?.(command, args);
    });

    const user = userEvent.setup();
    renderPage({ entryMode: "model-first" });
    const results = screen.getByRole("list", { name: "模型与供应商" });
    await user.click(within(results).getByRole("button", { name: /gpt-5\.6-sol.*OpenAI/ }));

    resolveCatalog({
      providers: { openai: ["gpt-current"] },
      source: "live",
      fetched_at_ms: 42,
      unavailable_provider_ids: [],
    });
    expect(await screen.findByRole("button", { name: /gpt-current/ }))
      .toHaveAttribute("aria-pressed", "false");
    await user.type(screen.getByLabelText("API Key"), "test-key");
    await user.click(screen.getByRole("button", { name: "添加供应商" }));

    expect(await screen.findByText("请至少选择一个模型。")).toBeInTheDocument();
    expect(vi.mocked(invoke)).not.toHaveBeenCalledWith(
      "add_provider_with_credential",
      expect.anything(),
    );
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
    expect(screen.queryByText("中国开放平台；与国际站 Key 不通用。")).not.toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "官方接入文档" })).not.toBeInTheDocument();
    expect(screen.getByText("MiniMax-M3")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /MiniMax-M3/ })).toHaveAttribute("aria-pressed", "false");
    expect(await screen.findByText("https://api.minimaxi.com/v1/chat/completions")).toBeInTheDocument();
    expect(screen.getByText("https://api.minimaxi.com/v1/responses")).toBeInTheDocument();
    expect(screen.getByText("https://api.minimaxi.com/v1/messages")).toBeInTheDocument();
  });

  it("disables the local-model flag for the DeepSeek cloud preset", async () => {
    window.localStorage.setItem("token-station-language", "zh-CN");
    const user = userEvent.setup();
    renderPage();

    await pickPreset(user, "DeepSeek");

    const local = screen.getByRole("checkbox", { name: /该模型在本机运行/ });
    expect(local).not.toBeChecked();
    expect(local).toBeDisabled();
    const eligibility = screen.getByText(/云端地址不能标记为本地模型/);
    expect(eligibility).not.toBeVisible();
    expect(local).toHaveAttribute("aria-describedby", eligibility.id);
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

    const local = screen.getByRole("checkbox", { name: /该模型在本机运行/ });
    expect(local).toBeChecked();
    expect(local).toBeEnabled();
    expect(screen.getByText(/已检测到本机回环地址/)).not.toBeVisible();
  });

  it("uses local store by default and submits env credentials as references only", async () => {
    window.localStorage.setItem("token-station-language", "zh-CN");
    const user = userEvent.setup();
    renderPage();
    await pickPreset(user, "DeepSeek");
    await pickModel(user, "deepseek-v4-flash");

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
