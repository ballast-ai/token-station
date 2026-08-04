import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import TierKeywords from "./TierKeywords";

const allConfigured = { high: true, mid: true, low: true };
const emptyKeywords = { high: [], mid: [], low: [] };

describe("TierKeywords", () => {
  it("keeps the priority tiers vertical in high-to-low DOM order without repeated empty copy", () => {
    render(
      <TierKeywords
        keywords={emptyKeywords}
        configured={allConfigured}
        onAdd={vi.fn()}
        onRemove={vi.fn()}
      />,
    );

    const tierGroups = ["强模型", "中模型", "弱模型"].map((name) =>
      screen.getByRole("group", { name }),
    );

    expect(tierGroups[0].compareDocumentPosition(tierGroups[1])).toBe(
      Node.DOCUMENT_POSITION_FOLLOWING,
    );
    expect(tierGroups[1].compareDocumentPosition(tierGroups[2])).toBe(
      Node.DOCUMENT_POSITION_FOLLOWING,
    );
    expect(screen.queryByText("这一档还没有关键词。添加关键词后，匹配的请求会固定到这一档。")).toBeNull();

    for (const group of tierGroups) {
      expect(within(group).getByRole("textbox")).toHaveAttribute(
        "placeholder",
        "输入关键词，回车加入",
      );
      expect(within(group).getByRole("button", { name: "添加" })).toBeDisabled();
    }
  });

  it("keeps each tier input independent and submits to the matching tier", async () => {
    const user = userEvent.setup();
    const onAdd = vi.fn();
    render(
      <TierKeywords
        keywords={emptyKeywords}
        configured={allConfigured}
        onAdd={onAdd}
        onRemove={vi.fn()}
      />,
    );

    const highTier = screen.getByRole("group", { name: "强模型" });
    await user.type(within(highTier).getByRole("textbox"), "架构设计");
    await user.click(within(highTier).getByRole("button", { name: "添加" }));

    expect(onAdd).toHaveBeenCalledWith("high", "架构设计");
    expect(within(highTier).getByRole("textbox")).toHaveValue("");
    expect(within(screen.getByRole("group", { name: "中模型" })).getByRole("textbox")).toHaveValue("");
  });

  it("keeps an unconfigured tier unavailable without adding another empty-state sentence", () => {
    render(
      <TierKeywords
        keywords={emptyKeywords}
        configured={{ high: true, mid: false, low: true }}
        onAdd={vi.fn()}
        onRemove={vi.fn()}
      />,
    );

    const midTier = screen.getByRole("group", { name: "中模型" });
    expect(within(midTier).getByRole("textbox")).toBeDisabled();
    expect(within(midTier).getByRole("textbox")).toHaveAttribute("placeholder", "该档未配置模型");
    expect(within(midTier).getByRole("button", { name: "添加" })).toBeDisabled();
    expect(screen.queryByText("先在上方为该档选好供应商和模型。")).toBeNull();
  });
});
