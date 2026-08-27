import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import TierKeywords from "./TierKeywords";

const allConfigured = { high: true, mid: true, low: true };
const emptyKeywords = { high: [], mid: [], low: [] };

describe("TierKeywords", () => {
  it("opens one tier editor with the same high-mid-low vocabulary as smart routing", () => {
    render(
      <TierKeywords
        keywords={emptyKeywords}
        configured={allConfigured}
        activeSlot="high"
        onOpenChange={vi.fn()}
        onAdd={vi.fn()}
        onRemove={vi.fn()}
      />,
    );

    const dialog = screen.getByRole("dialog", { name: "上档关键词" });
    expect(dialog).toHaveTextContent("命中后固定走上档，优先于自动判断");
    expect(screen.queryByText("强模型")).toBeNull();
    expect(screen.queryByText("中模型")).toBeNull();
    expect(screen.queryByText("弱模型")).toBeNull();
    expect(within(dialog).getByRole("textbox")).toHaveAttribute("placeholder", "输入关键词，回车加入");
    expect(within(dialog).getByRole("button", { name: "添加" })).toBeDisabled();
  });

  it("keeps each tier input independent and submits to the matching tier", async () => {
    const user = userEvent.setup();
    const onAdd = vi.fn();
    render(
      <TierKeywords
        keywords={emptyKeywords}
        configured={allConfigured}
        activeSlot="high"
        onOpenChange={vi.fn()}
        onAdd={onAdd}
        onRemove={vi.fn()}
      />,
    );

    const highTier = screen.getByRole("dialog", { name: "上档关键词" });
    await user.type(within(highTier).getByRole("textbox"), "架构设计");
    await user.click(within(highTier).getByRole("button", { name: "添加" }));

    expect(onAdd).toHaveBeenCalledWith("high", "架构设计");
    expect(within(highTier).getByRole("textbox")).toHaveValue("");
  });

  it("keeps an unconfigured tier unavailable without adding another empty-state sentence", () => {
    render(
      <TierKeywords
        keywords={emptyKeywords}
        configured={{ high: true, mid: false, low: true }}
        activeSlot="mid"
        onOpenChange={vi.fn()}
        onAdd={vi.fn()}
        onRemove={vi.fn()}
      />,
    );

    const midTier = screen.getByRole("dialog", { name: "中档关键词" });
    expect(within(midTier).getByRole("textbox")).toBeDisabled();
    expect(within(midTier).getByRole("textbox")).toHaveAttribute("placeholder", "该档未配置模型");
    expect(within(midTier).getByRole("button", { name: "添加" })).toBeDisabled();
    expect(screen.queryByText("先在上方为该档选好供应商和模型。")).toBeNull();
  });
});
