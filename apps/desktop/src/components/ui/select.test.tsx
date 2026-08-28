import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "./select";

describe("Select motion contract", () => {
  it("opens an animated listbox and preserves selection behavior", async () => {
    const user = userEvent.setup();
    const onValueChange = vi.fn();
    render(
      <Select defaultValue="system" onValueChange={onValueChange}>
        <SelectTrigger aria-label="Appearance">
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          <SelectGroup>
            <SelectItem value="light">Light</SelectItem>
            <SelectItem value="system">System</SelectItem>
          </SelectGroup>
        </SelectContent>
      </Select>,
    );

    const trigger = screen.getByRole("combobox", { name: "Appearance" });
    expect(trigger.querySelector("svg")).toHaveAttribute("data-motion-dropdown-chevron", "true");

    await user.click(trigger);
    expect(screen.getByRole("listbox")).toHaveAttribute("data-motion-dropdown", "select");
    expect(screen.getAllByRole("option")).toHaveLength(2);
    expect(screen.getByRole("listbox").querySelector('[data-motion-dropdown-list="true"]'))
      .toHaveClass("data-[position=popper]:min-h-(--radix-select-trigger-height)");
    expect(screen.getByRole("listbox").querySelector('[data-motion-dropdown-list="true"]'))
      .toHaveAttribute("data-position", "popper");

    await user.click(trigger);
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
    expect(document.querySelector('[data-motion-dropdown="select"]')).toBeNull();

    await user.click(trigger);
    expect(screen.getAllByRole("option")).toHaveLength(2);

    await user.click(screen.getByRole("option", { name: "Light" }));
    expect(onValueChange).toHaveBeenCalledWith("light");
    expect(trigger).toHaveTextContent("Light");
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
    expect(document.querySelector('[data-motion-dropdown="select"]')).toBeNull();
  });
});
