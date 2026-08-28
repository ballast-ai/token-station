import { render } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { usePageTransition } from "./use-page-transition";

function TransitionHarness({ view }: { view: string }) {
  const ref = usePageTransition<HTMLElement>(view);
  return <main ref={ref} data-testid="surface">{view}</main>;
}

const originalAnimate = HTMLElement.prototype.animate;
const originalMatchMedia = window.matchMedia;

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

describe("usePageTransition", () => {
  it("animates only after the transition key changes and cancels superseded motion", () => {
    const firstCancel = vi.fn();
    const secondCancel = vi.fn();
    const animate = vi
      .fn()
      .mockReturnValueOnce({ cancel: firstCancel } as unknown as Animation)
      .mockReturnValueOnce({ cancel: secondCancel } as unknown as Animation);
    Object.defineProperty(HTMLElement.prototype, "animate", {
      configurable: true,
      value: animate,
    });
    Object.defineProperty(window, "matchMedia", {
      configurable: true,
      value: vi.fn().mockReturnValue({ matches: false }),
    });

    const view = render(<TransitionHarness view="overview" />);
    expect(animate).not.toHaveBeenCalled();

    view.rerender(<TransitionHarness view="providers" />);
    expect(animate).toHaveBeenCalledWith(
      [
        { opacity: 0.68 },
        { opacity: 1 },
      ],
      {
        duration: 180,
        easing: "cubic-bezier(0.22, 1, 0.36, 1)",
        fill: "both",
      },
    );

    view.rerender(<TransitionHarness view="usage" />);
    expect(firstCancel).toHaveBeenCalledOnce();
    expect(animate).toHaveBeenCalledTimes(2);

    view.unmount();
    expect(secondCancel).toHaveBeenCalledOnce();
  });

  it("skips motion when the operating system requests reduced motion", () => {
    const animate = vi.fn();
    Object.defineProperty(HTMLElement.prototype, "animate", {
      configurable: true,
      value: animate,
    });
    Object.defineProperty(window, "matchMedia", {
      configurable: true,
      value: vi.fn().mockReturnValue({ matches: true }),
    });

    const view = render(<TransitionHarness view="overview" />);
    view.rerender(<TransitionHarness view="providers" />);

    expect(animate).not.toHaveBeenCalled();
  });
});
