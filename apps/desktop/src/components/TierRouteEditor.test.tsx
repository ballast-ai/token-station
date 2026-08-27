import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { ProviderView, TierSlot, TierView } from "../api";
import TierRouteEditor from "./TierRouteEditor";

const providers: ProviderView[] = [
  {
    name: "deepseek",
    brand_id: "deepseek",
    provider: "openai-compatible",
    base_url: "https://api.deepseek.com/v1",
    models: ["deepseek-v4-pro", "deepseek-v4-flash"],
    has_auth: true,
  },
  {
    name: "openai",
    brand_id: "openai",
    provider: "openai",
    base_url: "https://api.openai.com/v1",
    models: ["gpt-5.5", "gpt-5.5-mini"],
    has_auth: true,
  },
  {
    name: "empty",
    provider: "openai-compatible",
    base_url: "https://example.com/v1",
    models: [],
    has_auth: false,
  },
];

const tiers: Record<TierSlot, TierView> = {
  high: { upstream: "deepseek", model: "deepseek-v4-pro" },
  mid: { upstream: "openai", model: "gpt-5.5-mini" },
  low: { upstream: null, model: null },
};

describe("TierRouteEditor", () => {
  it("shows provider brands in the selected control and dropdown options", async () => {
    const user = userEvent.setup();
    render(
      <TierRouteEditor tiers={tiers} providers={providers} onTierChange={vi.fn()} />,
    );

    const trigger = screen.getByLabelText("上档供应商");
    expect(trigger.querySelector('[data-provider-brand="deepseek"]')).toBeInTheDocument();
    await user.click(trigger);
    expect(screen.getByRole("option", { name: "openai" })
      .querySelector('[data-provider-brand="openai"]')).toBeInTheDocument();
  });

  it("下拉框中自定义 openai 只显示首字母而不伪装官方图标", async () => {
    const user = userEvent.setup();
    const customProviders: ProviderView[] = [{
      name: "openai",
      brand_id: null,
      provider: "openai-compatible",
      base_url: "https://custom.example/v1",
      models: ["custom-model"],
      has_auth: true,
    }];
    render(
      <TierRouteEditor
        tiers={{
          high: { upstream: "openai", model: "custom-model" },
          mid: { upstream: null, model: null },
          low: { upstream: null, model: null },
        }}
        providers={customProviders}
        onTierChange={vi.fn()}
      />,
    );

    const trigger = screen.getByLabelText("上档供应商");
    expect(trigger.querySelector('[data-provider-brand="openai"]')).toBeNull();
    expect(trigger.querySelector(".brand-fallback")).toHaveTextContent("O");
    await user.click(trigger);
    const option = screen.getByRole("option", { name: "openai" });
    expect(option.querySelector('[data-provider-brand="openai"]')).toBeNull();
    expect(option.querySelector(".brand-fallback")).toHaveTextContent("O");
  });

  it("renders the existing three-tier structure with accessible controls", () => {
    render(
      <TierRouteEditor
        tiers={tiers}
        providers={providers}
        keywordCounts={{ high: 2, mid: 0, low: 1 }}
        onEditKeywords={vi.fn()}
        onTierChange={vi.fn()}
      />,
    );

    expect(screen.getByText("上档")).toBeInTheDocument();
    expect(screen.getByText("中档")).toBeInTheDocument();
    expect(screen.getByText("下档")).toBeInTheDocument();
    expect(screen.getByText("复杂推理与代码")).toBeInTheDocument();
    expect(screen.getByText("简单快速任务")).toBeInTheDocument();
    expect(screen.getByLabelText("上档供应商")).toHaveTextContent("deepseek");
    expect(screen.getByLabelText("上档模型")).toHaveTextContent("deepseek-v4-pro");
    expect(screen.getByLabelText("下档模型")).toBeDisabled();
    expect(screen.getByRole("button", { name: "编辑上档关键词，当前 2 个" })).toHaveTextContent("关键词 2");
    expect(screen.getByRole("button", { name: "编辑中档关键词，当前 0 个" })).toHaveTextContent("关键词");
    expect(screen.getByRole("button", { name: "编辑下档关键词，当前 1 个" })).toHaveTextContent("关键词 1");
    expect(screen.queryByRole("button", { name: "同步三档" })).not.toBeInTheDocument();
  });

  it("opens keyword editing for the matching tier", async () => {
    const user = userEvent.setup();
    const onEditKeywords = vi.fn();
    render(
      <TierRouteEditor
        tiers={tiers}
        providers={providers}
        keywordCounts={{ high: 0, mid: 3, low: 0 }}
        onEditKeywords={onEditKeywords}
        onTierChange={vi.fn()}
      />,
    );

    await user.click(screen.getByRole("button", { name: "编辑中档关键词，当前 3 个" }));
    expect(onEditKeywords).toHaveBeenCalledWith("mid");
  });

  it("selects the provider's first model when the provider changes", async () => {
    const onTierChange = vi.fn();
    const user = userEvent.setup();
    render(
      <TierRouteEditor tiers={tiers} providers={providers} onTierChange={onTierChange} />,
    );

    await user.click(screen.getByLabelText("上档供应商"));
    await user.click(screen.getByRole("option", { name: "openai" }));

    expect(onTierChange).toHaveBeenCalledWith("high", "openai", "gpt-5.5");
  });

  it("supports keyboard navigation in the provider menu", async () => {
    const onTierChange = vi.fn();
    const user = userEvent.setup();
    render(
      <TierRouteEditor tiers={tiers} providers={providers} onTierChange={onTierChange} />,
    );

    const trigger = screen.getByLabelText("上档供应商");
    await user.click(trigger);
    await user.keyboard("{ArrowDown}{ArrowDown}{Enter}");

    expect(onTierChange).toHaveBeenCalledWith("high", "openai", "gpt-5.5");
  });

  it("places the selected provider first for each tier", async () => {
    const user = userEvent.setup();
    render(
      <TierRouteEditor tiers={tiers} providers={providers} onTierChange={vi.fn()} />,
    );

    await user.click(screen.getByLabelText("上档供应商"));
    expect(screen.getAllByRole("option").map((option) => option.getAttribute("title"))).toEqual([
      "deepseek",
      "未选择",
      "openai",
      "empty",
    ]);

    await user.keyboard("{Escape}");
    await user.click(screen.getByLabelText("中档供应商"));
    expect(screen.getAllByRole("option").map((option) => option.getAttribute("title"))).toEqual([
      "openai",
      "未选择",
      "deepseek",
      "empty",
    ]);
  });

  it("uses an empty model when the selected provider has no models", async () => {
    const onTierChange = vi.fn();
    const user = userEvent.setup();
    render(
      <TierRouteEditor tiers={tiers} providers={providers} onTierChange={onTierChange} />,
    );

    await user.click(screen.getByLabelText("中档供应商"));
    await user.click(screen.getByRole("option", { name: "empty" }));

    expect(onTierChange).toHaveBeenCalledWith("mid", "empty", null);
  });

  it("supports clearing the provider and model independently", async () => {
    const onTierChange = vi.fn();
    const user = userEvent.setup();
    render(
      <TierRouteEditor tiers={tiers} providers={providers} onTierChange={onTierChange} />,
    );

    await user.click(screen.getByLabelText("上档供应商"));
    await user.click(screen.getByRole("option", { name: "未选择" }));
    await user.click(screen.getByLabelText("中档模型"));
    await user.click(screen.getByRole("option", { name: "未选择" }));

    expect(onTierChange).toHaveBeenNthCalledWith(1, "high", null, null);
    expect(onTierChange).toHaveBeenNthCalledWith(2, "mid", "openai", null);
  });

  it("flags a deleted provider or removed model as unavailable instead of showing it as valid", () => {
    const staleTiers: Record<TierSlot, TierView> = {
      high: { upstream: "gone-provider", model: "gone-model" }, // provider deleted from pool
      mid: { upstream: "deepseek", model: "removed-model" }, // provider ok, model removed
      low: { upstream: null, model: null },
    };
    render(
      <TierRouteEditor tiers={staleTiers} providers={providers} onTierChange={vi.fn()} />,
    );

    // The stale selection is still shown (so the user sees what was set) but flagged.
    const staleProvider = screen.getByLabelText("上档供应商");
    expect(staleProvider).toHaveTextContent("gone-provider");
    expect(staleProvider).toHaveTextContent("已失效");
    expect(screen.getByLabelText("中档模型")).toHaveTextContent("已失效");
    // A valid selection is untouched — no false "unavailable" flag.
    expect(screen.getByLabelText("中档供应商")).not.toHaveTextContent("已失效");
  });

  it.each([
    { disabled: true, readOnly: false },
    { disabled: false, readOnly: true },
  ])("freezes every control for $disabled/$readOnly mode", ({ disabled, readOnly }) => {
    render(
      <TierRouteEditor
        tiers={tiers}
        providers={providers}
        disabled={disabled}
        readOnly={readOnly}
        onTierChange={vi.fn()}
      />,
    );

    for (const control of screen.getAllByRole("combobox")) {
      expect(control).toBeDisabled();
    }
  });
});
