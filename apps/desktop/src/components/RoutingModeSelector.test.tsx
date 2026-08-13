import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import RoutingModeSelector from "./RoutingModeSelector";

describe("RoutingModeSelector", () => {
  it("presents Direct first and emits the public direct mode", async () => {
    const onValueChange = vi.fn();
    const user = userEvent.setup();
    render(
      <RoutingModeSelector value="tiered" onValueChange={onValueChange} />,
    );

    const tabs = within(screen.getByRole("tablist", { name: "路由模式" })).getAllByRole("tab");
    expect(tabs.map((tab) => tab.getAttribute("aria-label"))).toEqual([
      "单独路由",
      "智能分档",
      "额度优先",
    ]);

    await user.click(tabs[0]);
    expect(onValueChange).toHaveBeenCalledWith("direct");
  });
});
