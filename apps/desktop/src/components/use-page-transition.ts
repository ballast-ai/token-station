import { useLayoutEffect, useRef, type RefObject } from "react";

const PAGE_TRANSITION_KEYFRAMES: Keyframe[] = [
  { opacity: 0.68 },
  { opacity: 1 },
];

const PAGE_TRANSITION_OPTIONS: KeyframeAnimationOptions = {
  duration: 180,
  easing: "cubic-bezier(0.22, 1, 0.36, 1)",
  fill: "both",
};

function prefersReducedMotion(): boolean {
  return typeof window.matchMedia === "function"
    && window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

export function usePageTransition<T extends HTMLElement>(transitionKey: string): RefObject<T | null> {
  const surfaceRef = useRef<T | null>(null);
  const previousKeyRef = useRef(transitionKey);
  const animationRef = useRef<Animation | null>(null);

  useLayoutEffect(() => {
    if (previousKeyRef.current === transitionKey) return;
    previousKeyRef.current = transitionKey;

    animationRef.current?.cancel();
    animationRef.current = null;

    const surface = surfaceRef.current;
    if (!surface || typeof surface.animate !== "function" || prefersReducedMotion()) return;

    animationRef.current = surface.animate(PAGE_TRANSITION_KEYFRAMES, PAGE_TRANSITION_OPTIONS);
  }, [transitionKey]);

  useLayoutEffect(() => () => {
    animationRef.current?.cancel();
  }, []);

  return surfaceRef;
}
