import { useLayoutEffect, useRef } from "react";

type TransitionTarget = "self" | "first-child";

const CONTENT_TRANSITION_KEYFRAMES: Keyframe[] = [
  { opacity: 0.72, transform: "translateY(5px)" },
  { opacity: 1, transform: "translateY(0)" },
];

const CONTENT_TRANSITION_OPTIONS: KeyframeAnimationOptions = {
  duration: 180,
  easing: "cubic-bezier(0.22, 1, 0.36, 1)",
  fill: "both",
};

function prefersReducedMotion(): boolean {
  return window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;
}

export function useContentTransition<T extends HTMLElement>(
  transitionKey: string,
  target: TransitionTarget = "self",
) {
  const hostRef = useRef<T>(null);
  const previousKeyRef = useRef(transitionKey);
  const activeAnimationRef = useRef<Animation | null>(null);

  useLayoutEffect(() => {
    if (previousKeyRef.current === transitionKey) return undefined;
    previousKeyRef.current = transitionKey;

    activeAnimationRef.current?.cancel();
    activeAnimationRef.current = null;

    const host = hostRef.current;
    const element = target === "first-child" ? host?.firstElementChild : host;
    if (
      !(element instanceof HTMLElement)
      || prefersReducedMotion()
      || typeof element.animate !== "function"
    ) {
      return undefined;
    }

    const animation = element.animate(
      CONTENT_TRANSITION_KEYFRAMES,
      CONTENT_TRANSITION_OPTIONS,
    );
    activeAnimationRef.current = animation;
    animation.onfinish = () => {
      if (activeAnimationRef.current !== animation) return;
      activeAnimationRef.current = null;
      animation.cancel();
    };

    return () => {
      if (activeAnimationRef.current === animation) {
        activeAnimationRef.current = null;
      }
      animation.cancel();
    };
  }, [target, transitionKey]);

  return hostRef;
}
