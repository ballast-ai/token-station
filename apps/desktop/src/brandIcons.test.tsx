import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { AgentIcon } from "./brandIcons";

describe("AgentIcon", () => {
  it("uses the bundled WorkBuddy app icon instead of the WB placeholder", () => {
    const { container } = render(<AgentIcon id="workbuddy" fallback="WB" size={22} />);

    const image = container.querySelector("img");
    expect(image).not.toBeNull();
    expect(image).toHaveAttribute("src", "/agents/workbuddy.png");
    expect(image).toHaveClass("brand-image-app");
    expect(screen.queryByText("WB")).toBeNull();
  });
});
