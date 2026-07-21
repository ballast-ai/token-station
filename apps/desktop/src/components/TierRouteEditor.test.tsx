import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { ProviderView, TierSlot, TierView } from "../api";
import TierRouteEditor from "./TierRouteEditor";

const providers: ProviderView[] = [
  {
    name: "deepseek",
    provider: "openai-compatible",
    base_url: "https://api.deepseek.com/v1",
    models: ["deepseek-v4-pro", "deepseek-v4-flash"],
    has_auth: true,
  },
  {
    name: "openai",
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
  it("renders the existing three-tier structure with accessible controls", () => {
    render(
      <TierRouteEditor tiers={tiers} providers={providers} onTierChange={vi.fn()} />,
    );

    expect(screen.getByText("上档")).toBeInTheDocument();
    expect(screen.getByText("中档")).toBeInTheDocument();
    expect(screen.getByText("下档")).toBeInTheDocument();
    expect(screen.getByText("最强模型 · 难任务升到这里")).toBeInTheDocument();
    expect(screen.getByText("便宜快模型 · 简单任务兜底")).toBeInTheDocument();
    expect(screen.getByLabelText("上档供应商")).toHaveValue("deepseek");
    expect(screen.getByLabelText("上档模型")).toHaveValue("deepseek-v4-pro");
    expect(screen.getByLabelText("下档模型")).toBeDisabled();
  });

  it("selects the provider's first model when the provider changes", async () => {
    const onTierChange = vi.fn();
    const user = userEvent.setup();
    render(
      <TierRouteEditor tiers={tiers} providers={providers} onTierChange={onTierChange} />,
    );

    await user.selectOptions(screen.getByLabelText("上档供应商"), "openai");

    expect(onTierChange).toHaveBeenCalledWith("high", "openai", "gpt-5.5");
  });

  it("uses an empty model when the selected provider has no models", async () => {
    const onTierChange = vi.fn();
    const user = userEvent.setup();
    render(
      <TierRouteEditor tiers={tiers} providers={providers} onTierChange={onTierChange} />,
    );

    await user.selectOptions(screen.getByLabelText("中档供应商"), "empty");

    expect(onTierChange).toHaveBeenCalledWith("mid", "empty", null);
  });

  it("supports clearing the provider and model independently", async () => {
    const onTierChange = vi.fn();
    const user = userEvent.setup();
    render(
      <TierRouteEditor tiers={tiers} providers={providers} onTierChange={onTierChange} />,
    );

    await user.selectOptions(screen.getByLabelText("上档供应商"), "");
    await user.selectOptions(screen.getByLabelText("中档模型"), "");

    expect(onTierChange).toHaveBeenNthCalledWith(1, "high", null, null);
    expect(onTierChange).toHaveBeenNthCalledWith(2, "mid", "openai", null);
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
