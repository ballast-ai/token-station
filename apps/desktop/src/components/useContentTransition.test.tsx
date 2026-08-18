import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { useContentTransition } from "./useContentTransition";

const originalAnimate = HTMLElement.prototype.animate;
const originalMatchMedia = window.matchMedia;

function Harness({ transitionKey }: { transitionKey: string }) {
  const ref = useContentTransition<HTMLDivElement>(transitionKey, "first-child");
  return (
    <div ref={ref} data-testid="transition-parent">
      <section data-testid="transition-target">{transitionKey}</section>
    </div>
  );
}

afterEach(() => {
  if (originalAnimate) {
    Object.defineProperty(HTMLElement.prototype, "animate", {
      configurable: true,
      value: originalAnimate,
    });
  } else {
    Reflect.deleteProperty(HTMLElement.prototype, "animate");
  }
  Object.defineProperty(window, "matchMedia", {
    configurable: true,
    value: originalMatchMedia,
  });
});

describe("content transitions", () => {
  it("animates only after the key changes and cancels superseded motion", async () => {
    const firstCancel = vi.fn();
    const secondCancel = vi.fn();
    const animate = vi.fn()
      .mockReturnValueOnce({ cancel: firstCancel } as unknown as Animation)
      .mockReturnValueOnce({ cancel: secondCancel } as unknown as Animation);
    Object.defineProperty(HTMLElement.prototype, "animate", {
      configurable: true,
      value: animate,
    });
    const user = userEvent.setup();

    function InteractiveHarness() {
      const [transitionKey, setTransitionKey] = useState("home");
      return (
        <>
          <button type="button" onClick={() => setTransitionKey("providers")}>Providers</button>
          <button type="button" onClick={() => setTransitionKey("settings")}>Settings</button>
          <Harness transitionKey={transitionKey} />
        </>
      );
    }

    render(<InteractiveHarness />);
    expect(animate).not.toHaveBeenCalled();

    await user.click(screen.getByRole("button", { name: "Providers" }));
    expect(animate).toHaveBeenCalledTimes(1);
    expect(animate.mock.instances[0]).toBe(screen.getByTestId("transition-target"));
    expect(animate).toHaveBeenLastCalledWith(
      [
        { opacity: 0.72, transform: "translateY(5px)" },
        { opacity: 1, transform: "translateY(0)" },
      ],
      { duration: 180, easing: "cubic-bezier(0.22, 1, 0.36, 1)", fill: "both" },
    );

    await user.click(screen.getByRole("button", { name: "Settings" }));
    expect(firstCancel).toHaveBeenCalledTimes(1);
    expect(animate).toHaveBeenCalledTimes(2);
  });

  it("skips motion when the user requests reduced motion", async () => {
    const animate = vi.fn();
    Object.defineProperty(HTMLElement.prototype, "animate", {
      configurable: true,
      value: animate,
    });
    Object.defineProperty(window, "matchMedia", {
      configurable: true,
      value: vi.fn().mockReturnValue({ matches: true }),
    });
    const { rerender } = render(<Harness transitionKey="home" />);

    rerender(<Harness transitionKey="providers" />);

    expect(animate).not.toHaveBeenCalled();
  });
});
