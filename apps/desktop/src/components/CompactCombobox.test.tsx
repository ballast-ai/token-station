import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import CompactCombobox from "./CompactCombobox";

const OPTIONS = [
  { value: "", label: "未选择" },
  { value: "deepseek", label: "deepseek" },
  { value: "minimax", label: "minimax_cn" },
  { value: "openrouter", label: "openrouter" },
];

describe("CompactCombobox", () => {
  it("keeps the user's active option when its parent rerenders", async () => {
    const user = userEvent.setup();
    const { rerender } = render(
      <CompactCombobox ariaLabel="供应商" value="deepseek" options={[...OPTIONS]} onChange={vi.fn()} />,
    );

    await user.click(screen.getByRole("combobox", { name: "供应商" }));
    await user.keyboard("{ArrowDown}");
    expect(screen.getByRole("option", { name: "minimax_cn" })).toHaveFocus();

    rerender(
      <CompactCombobox ariaLabel="供应商" value="deepseek" options={[...OPTIONS]} onChange={vi.fn()} />,
    );

    expect(screen.getByRole("option", { name: "minimax_cn" })).toHaveFocus();
  });

  it("returns focus to the trigger after Escape", async () => {
    const user = userEvent.setup();
    render(
      <CompactCombobox ariaLabel="供应商" value="deepseek" options={OPTIONS} onChange={vi.fn()} />,
    );

    const trigger = screen.getByRole("combobox", { name: "供应商" });
    await user.click(trigger);
    await user.keyboard("{Escape}");

    expect(trigger).toHaveFocus();
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
  });

  it("closes without emitting a change when the selected option is chosen again", async () => {
    const onChange = vi.fn();
    const user = userEvent.setup();
    render(
      <CompactCombobox ariaLabel="供应商" value="deepseek" options={OPTIONS} onChange={onChange} />,
    );

    await user.click(screen.getByRole("combobox", { name: "供应商" }));
    await user.click(screen.getByRole("option", { name: "deepseek" }));

    expect(onChange).not.toHaveBeenCalled();
    expect(screen.getByRole("combobox", { name: "供应商" })).toHaveFocus();
  });

  it("closes an open menu when the control becomes disabled", async () => {
    const user = userEvent.setup();
    const { rerender } = render(
      <CompactCombobox ariaLabel="供应商" value="" options={OPTIONS} onChange={vi.fn()} />,
    );

    await user.click(screen.getByRole("combobox", { name: "供应商" }));
    expect(screen.getByRole("listbox")).toBeInTheDocument();

    rerender(
      <CompactCombobox ariaLabel="供应商" value="" options={OPTIONS} disabled onChange={vi.fn()} />,
    );

    expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
  });

  it("focuses a selected item that is beyond the initial large-list window", async () => {
    const user = userEvent.setup();
    const options = Array.from({ length: 140 }, (_, index) => ({
      value: `model-${index}`,
      label: `model-${index}`,
    }));
    render(
      <CompactCombobox ariaLabel="模型" value="model-130" options={options} onChange={vi.fn()} />,
    );

    await user.click(screen.getByRole("combobox", { name: "模型" }));

    expect(screen.getByRole("option", { name: "model-130" })).toHaveFocus();
    expect(screen.getByLabelText("搜索模型")).toBeInTheDocument();
  });

  it("anchors an upward menu to the trigger instead of its maximum height", async () => {
    const user = userEvent.setup();
    const originalInnerHeight = window.innerHeight;
    const rectSpy = vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue({
      x: 32,
      y: 520,
      top: 520,
      right: 472,
      bottom: 560,
      left: 32,
      width: 440,
      height: 40,
      toJSON: () => ({}),
    });
    Object.defineProperty(window, "innerHeight", { configurable: true, value: 600 });

    render(
      <CompactCombobox ariaLabel="供应商" value="minimax" options={OPTIONS} onChange={vi.fn()} />,
    );
    await user.click(screen.getByRole("combobox", { name: "供应商" }));

    expect(screen.getByRole("listbox").parentElement).toHaveStyle({
      bottom: "86px",
      top: "auto",
    });
    rectSpy.mockRestore();
    Object.defineProperty(window, "innerHeight", { configurable: true, value: originalInnerHeight });
  });
});
