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

  it("scrolls the trigger into view and keeps a downward menu inside the viewport", async () => {
    const user = userEvent.setup();
    const originalInnerHeight = window.innerHeight;
    const originalScrollIntoView = HTMLElement.prototype.scrollIntoView;
    const scrollIntoView = vi.fn();
    Object.defineProperty(HTMLElement.prototype, "scrollIntoView", {
      configurable: true,
      value: scrollIntoView,
    });
    Object.defineProperty(window, "innerHeight", { configurable: true, value: 600 });

    render(
      <CompactCombobox ariaLabel="供应商" value="minimax" options={OPTIONS} onChange={vi.fn()} />,
    );
    const trigger = screen.getByRole("combobox", { name: "供应商" });
    const rectSpy = vi.spyOn(trigger, "getBoundingClientRect").mockReturnValueOnce({
      x: 32,
      y: 520,
      top: 520,
      right: 472,
      bottom: 560,
      left: 32,
      width: 440,
      height: 40,
      toJSON: () => ({}),
    }).mockReturnValue({
      x: 32,
      y: 380,
      top: 380,
      right: 472,
      bottom: 420,
      left: 32,
      width: 440,
      height: 40,
      toJSON: () => ({}),
    });

    try {
      await user.click(trigger);

      expect(scrollIntoView).toHaveBeenCalledWith({ block: "nearest", inline: "nearest" });
      const popover = screen.getByRole("listbox").parentElement;
      expect(popover).toHaveStyle({
        bottom: "auto",
        top: "426px",
        maxHeight: "162px",
      });
      expect(
        Number.parseFloat(popover?.style.top ?? "0")
          + Number.parseFloat(popover?.style.maxHeight ?? "0"),
      ).toBeLessThanOrEqual(window.innerHeight - 12);
    } finally {
      rectSpy.mockRestore();
      if (originalScrollIntoView) {
        Object.defineProperty(HTMLElement.prototype, "scrollIntoView", {
          configurable: true,
          value: originalScrollIntoView,
        });
      } else {
        delete (HTMLElement.prototype as Partial<HTMLElement>).scrollIntoView;
      }
      Object.defineProperty(window, "innerHeight", { configurable: true, value: originalInnerHeight });
    }
  });
});
