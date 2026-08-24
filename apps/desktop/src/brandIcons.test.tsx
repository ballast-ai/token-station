import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { AgentIcon, ProviderIcon } from "./brandIcons";

describe("AgentIcon", () => {
  it.each([
    ["grok-build", "G"],
    ["kimi-code", "K"],
    ["deepseek-harness", "D"],
  ])("uses an official brand glyph for %s", (id, fallback) => {
    const { container } = render(<AgentIcon id={id} fallback={fallback} size={22} />);

    expect(container.querySelector("svg")).toBeInTheDocument();
    expect(screen.queryByText(fallback)).toBeNull();
  });

  it("fits Kimi's white color mark inside the responsive Agent slot", () => {
    const { container } = render(<AgentIcon id="kimi-code" fallback="K" size={22} />);
    const icon = container.querySelector('[data-agent-brand="kimi-code"]');
    const avatar = icon?.querySelector<HTMLElement>('[data-kimi-avatar="true"]');
    const glyph = avatar?.querySelector<SVGElement>("svg");

    expect(avatar).toHaveStyle({ width: "100%", height: "100%", background: "#000" });
    expect(glyph).toHaveStyle({ width: "62%", height: "62%" });
    expect(glyph?.querySelector('path[fill="#fff"]')).toBeInTheDocument();
  });

  it("hides decorative Agent artwork from assistive technology", () => {
    const { container } = render(<AgentIcon id="grok-build" fallback="G" size={22} />);

    expect(container.querySelector('[data-agent-brand="grok-build"]')).toHaveAttribute("aria-hidden", "true");
  });

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
    expect(icon?.querySelector("svg")).toHaveClass("provider-brand-avatar-icon", "size-full");
  });

  it("uses a self-contained fallback for unknown brand identifiers", () => {
    const { container } = render(<ProviderIcon id="unknown-brand" label="Example" size={34} />);

    expect(container.querySelector("[data-provider-brand]")).toBeNull();
    const fallback = screen.getByText("E", { selector: ".brand-fallback" });
    expect(fallback).toHaveAttribute("aria-hidden", "true");
  });
});
