import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { describe, expect, it, vi } from "vitest";
import type { FreeProviderPresetView } from "../api";
import AddProviderPage, {
  type FreeCatalogFilters,
  type ProviderCatalogMode,
  type RegularCatalogFilters,
} from "./AddProviderPage";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async (command: string) => {
    if (command === "preview_provider_endpoints") {
      return {
        chat: "https://api.minimaxi.com/v1/chat/completions",
        responses: "https://api.minimaxi.com/v1/responses",
        messages: "https://api.minimaxi.com/v1/messages",
      };
    }
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

describe("AddProviderPage", () => {
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

  it("offers official and self-hosted presets as cards and omits non-default aggregators", () => {
    renderPage();

    const grid = screen.getByRole("list", { name: "常规供应商列表" });
    // Representatives of official usage-based APIs and local self-hosted providers both appear as cards.
    expect(within(grid).getByText("MiniMax（中国）")).toBeInTheDocument();
    expect(within(grid).getByText("本地 Ollama")).toBeInTheDocument();
    // Aggregator candidates are excluded from the default catalog and do not appear as cards.
    expect(screen.queryByText(/OpenRouter/)).toBeNull();
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

    await user.type(screen.getByRole("searchbox", { name: "搜索免费供应商" }), "gpt-oss");
    const grid = screen.getByRole("list", { name: "免费供应商列表" });
    expect(within(grid).getByText("NVIDIA API Catalog")).toBeInTheDocument();
    expect(within(grid).queryByText("Cohere")).toBeNull();
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
