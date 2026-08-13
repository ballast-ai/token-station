import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { AgentIcon, ProviderIcon } from "./brandIcons";

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

describe("ProviderIcon", () => {
  it("marks only resolved official brands and hides decorative artwork", () => {
    const { container } = render(<ProviderIcon id="deepseek" label="DeepSeek" size={34} />);

    const icon = container.querySelector('[data-provider-brand="deepseek"]');
    expect(icon).toHaveAttribute("aria-hidden", "true");
    expect(icon?.querySelector("svg")).toBeInTheDocument();
  });

  it("uses a self-contained fallback for unknown brand identifiers", () => {
    const { container } = render(<ProviderIcon id="unknown-brand" label="Example" size={34} />);

    expect(container.querySelector("[data-provider-brand]")).toBeNull();
    const fallback = screen.getByText("E", { selector: ".brand-fallback" });
    expect(fallback).toHaveAttribute("aria-hidden", "true");
  });
});
