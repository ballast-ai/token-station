import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import TierKeywords from "./TierKeywords";

describe("TierKeywords", () => {
  it("renders one continuous inline editor for every smart-routing tier", () => {
    render(
      <>
        <TierKeywords slot="high" keywords={["架构"]} configured onAdd={vi.fn()} onRemove={vi.fn()} />
        <TierKeywords slot="mid" keywords={[]} configured onAdd={vi.fn()} onRemove={vi.fn()} />
        <TierKeywords slot="low" keywords={["摘要"]} configured onAdd={vi.fn()} onRemove={vi.fn()} />
      </>,
    );

    const highTier = screen.getByRole("group", { name: "上档关键词" });
    expect(within(highTier).getByText("架构")).toBeInTheDocument();
    expect(within(highTier).getByRole("textbox", { name: "上档关键词" }))
      .toHaveAttribute("placeholder", "输入关键词后按回车");
    expect(screen.getByRole("group", { name: "中档关键词" })).toBeInTheDocument();
    expect(screen.getByRole("group", { name: "下档关键词" })).toBeInTheDocument();
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(screen.queryByRole("button", { name: "添加" })).toBeNull();
  });

  it("submits on Return, clears the draft, and keeps the input focused for repeated entry", async () => {
    const user = userEvent.setup();
    const onAdd = vi.fn();
    render(
      <TierKeywords
        slot="high"
        keywords={[]}
        configured
        onAdd={onAdd}
        onRemove={vi.fn()}
      />,
    );

    const input = screen.getByRole("textbox", { name: "上档关键词" });
    await user.type(input, "架构设计{Enter}");

    expect(onAdd).toHaveBeenCalledWith("high", "架构设计");
    expect(input).toHaveValue("");
    expect(input).toHaveFocus();
  });

  it("keeps configured keywords removable without highlighting them", async () => {
    const user = userEvent.setup();
    const onRemove = vi.fn();
    render(
      <TierKeywords
        slot="high"
        keywords={["代码", "推理"]}
        configured
        onAdd={vi.fn()}
        onRemove={onRemove}
      />,
    );

    const highTier = screen.getByRole("group", { name: "上档关键词" });
    expect(within(highTier).getByRole("list")).toHaveAttribute("data-presentation", "plain-text");
    await user.click(within(highTier).getByRole("button", { name: "删除关键词 代码" }));

    expect(onRemove).toHaveBeenCalledWith("high", "代码");
  });

  it("reports a duplicate inline without calling the backend or clearing the draft", async () => {
    const user = userEvent.setup();
    const onAdd = vi.fn();
    render(
      <TierKeywords
        slot="high"
        keywords={["Architecture"]}
        configured
        onAdd={onAdd}
        onRemove={vi.fn()}
      />,
    );

    const input = screen.getByRole("textbox", { name: "上档关键词" });
    await user.type(input, "architecture{Enter}");

    expect(onAdd).not.toHaveBeenCalled();
    expect(input).toHaveValue("architecture");
    expect(input).toHaveAttribute("aria-invalid", "true");
    expect(screen.getByRole("alert")).toHaveTextContent("关键词“architecture”已存在");
  });

  it("keeps the draft when the add operation is rejected", async () => {
    const user = userEvent.setup();
    render(
      <TierKeywords
        slot="high"
        keywords={[]}
        configured
        onAdd={vi.fn().mockResolvedValue(false)}
        onRemove={vi.fn()}
      />,
    );

    const input = screen.getByRole("textbox", { name: "上档关键词" });
    await user.type(input, "待重试{Enter}");

    expect(input).toHaveValue("待重试");
  });

  it("disables only the input for an unconfigured tier", () => {
    render(
      <TierKeywords
        slot="mid"
        keywords={[]}
        configured={false}
        onAdd={vi.fn()}
        onRemove={vi.fn()}
      />,
    );

    expect(screen.getByRole("textbox", { name: "中档关键词" })).toBeDisabled();
    expect(screen.getByRole("textbox", { name: "中档关键词" }))
      .toHaveAttribute("placeholder", "请先选择供应商和模型");
  });
});
