import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { expect, it, vi } from "vitest";
import FirstRunGuide from "./FirstRunGuide";
import { LanguageProvider } from "./LanguageProvider";
import TierRouteEditor from "./TierRouteEditor";

function rect(top: number, left: number, width: number, height: number): DOMRect {
  return {
    x: left,
    y: top,
    top,
    left,
    right: left + width,
    bottom: top + height,
    width,
    height,
    toJSON: () => ({}),
  };
}

it("keeps the route spotlight aligned when a rejected scroll is restored", async () => {
  const getBoundingClientRect = vi
    .spyOn(HTMLElement.prototype, "getBoundingClientRect")
    .mockImplementation(function getBoundingClientRectMock(this: HTMLElement) {
      if (this.getAttribute("data-onboarding-target") === "route-config") {
        const workspace = this.closest<HTMLElement>(".station-content");
        return rect(100 - (workspace?.scrollTop ?? 0), 100, 800, 400);
      }
      return rect(0, 0, 0, 0);
    });

  try {
    render(
      <LanguageProvider>
        <main className="station-content">
          <section data-onboarding-target="route-config">
            <button type="button">上档</button>
            <button type="button">中档</button>
            <button type="button">下档</button>
          </section>
        </main>
        <FirstRunGuide
          open
          microStep="route-config"
          scanBusy={false}
          onTargetAction={() => {}}
          onBack={() => {}}
          onSkipAgent={() => {}}
          onPause={() => {}}
          onDismiss={() => {}}
        />
      </LanguageProvider>,
    );

    await screen.findByRole("dialog", { name: "配置模型路由" });
    const workspace = document.querySelector<HTMLElement>(".station-content");
    const outline = document.querySelector<HTMLElement>(".first-run-spotlight-outline");
    expect(workspace).not.toBeNull();
    expect(outline).not.toBeNull();
    await waitFor(() => {
      expect(outline).toHaveStyle({ top: "93px", height: "414px" });
    });

    act(() => {
      workspace!.scrollTop = 220;
      workspace!.dispatchEvent(new Event("scroll"));
    });

    expect(workspace!.scrollTop).toBe(0);
    await waitFor(() => {
      expect(outline).toHaveStyle({ top: "93px", height: "414px" });
    });
  } finally {
    getBoundingClientRect.mockRestore();
  }
});

it("keeps all three tiers highlighted after replacing an existing model", async () => {
  const getBoundingClientRect = vi
    .spyOn(HTMLElement.prototype, "getBoundingClientRect")
    .mockImplementation(function getBoundingClientRectMock(this: HTMLElement) {
      if (this.getAttribute("data-onboarding-target") === "route-config") {
        const workspace = this.closest<HTMLElement>(".station-content");
        return rect(100 - (workspace?.scrollTop ?? 0), 100, 800, 400);
      }
      if (this.getAttribute("aria-label") === "下档模型") {
        return rect(620, 500, 300, 48);
      }
      return rect(0, 0, 0, 0);
    });
  const scrollIntoView = vi
    .spyOn(HTMLElement.prototype, "scrollIntoView")
    .mockImplementation(function scrollIntoViewMock(this: HTMLElement) {
      if (this.getAttribute("aria-label") !== "下档模型") return;
      const workspace = this.closest<HTMLElement>(".station-content");
      if (!workspace) return;
      workspace.scrollTop = 220;
      workspace.dispatchEvent(new Event("scroll"));
    });

  function Harness() {
    const [tiers, setTiers] = useState({
      high: { upstream: "deepseek", model: "deepseek-v4-flash" },
      mid: { upstream: "deepseek", model: "deepseek-v4-flash" },
      low: { upstream: "deepseek", model: "deepseek-v4-flash" },
    });
    return (
      <LanguageProvider>
        <main className="station-content">
          <section data-onboarding-target="route-config">
            <TierRouteEditor
              tiers={tiers}
              providers={[{
                name: "deepseek",
                provider: "openai-compatible",
                base_url: "https://api.deepseek.com/v1",
                models: ["deepseek-v4-flash", "deepseek-reasoner"],
                has_auth: true,
              }]}
              onTierChange={(slot, upstream, model) => {
                setTiers((current) => ({ ...current, [slot]: { upstream, model } }));
              }}
            />
          </section>
        </main>
        <FirstRunGuide
          open
          microStep="route-config"
          scanBusy={false}
          onTargetAction={() => {}}
          onBack={() => {}}
          onSkipAgent={() => {}}
          onPause={() => {}}
          onDismiss={() => {}}
        />
      </LanguageProvider>
    );
  }

  try {
    const user = userEvent.setup();
    render(<Harness />);
    await screen.findByRole("dialog", { name: "配置模型路由" });
    const workspace = document.querySelector<HTMLElement>(".station-content");
    const outline = document.querySelector<HTMLElement>(".first-run-spotlight-outline");
    expect(workspace).not.toBeNull();
    expect(outline).not.toBeNull();
    await waitFor(() => {
      expect(outline).toHaveStyle({ top: "93px", height: "414px" });
    });

    await user.click(screen.getByRole("combobox", { name: "下档模型" }));
    await user.click(screen.getByRole("option", { name: "deepseek-reasoner" }));

    expect(screen.getByRole("combobox", { name: "下档模型" }))
      .toHaveTextContent("deepseek-reasoner");
    expect(workspace!.scrollTop).toBe(0);
    expect(outline).toHaveStyle({ top: "93px", height: "414px" });
  } finally {
    scrollIntoView.mockRestore();
    getBoundingClientRect.mockRestore();
  }
});
