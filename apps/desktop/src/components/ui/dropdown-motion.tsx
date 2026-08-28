import type { ReactNode } from "react";
import {
  motion,
  useReducedMotion,
  type HTMLMotionProps,
  type Transition,
  type Variants,
} from "motion/react";
import "./dropdown-motion.css";

export const DROPDOWN_CHEVRON_TRANSITION: Transition = {
  type: "spring",
  duration: 0.4,
  bounce: 0.3,
};

export const DROPDOWN_LIST_VARIANTS: Variants = {
  closed: {},
  open: { transition: { delayChildren: 0.05, staggerChildren: 0.035 } },
};

export const DROPDOWN_ITEM_VARIANTS: Variants = {
  closed: { opacity: 0, y: -6, filter: "blur(3px)" },
  open: { opacity: 1, y: 0, filter: "blur(0px)" },
};

export const REDUCED_DROPDOWN_ITEM_VARIANTS: Variants = {
  closed: { opacity: 1, y: 0, filter: "blur(0px)" },
  open: { opacity: 1, y: 0, filter: "blur(0px)" },
};

export const DROPDOWN_SURFACE_VARIANTS: Variants = {
  closed: {
    opacity: 0,
    scaleY: 0.94,
    y: -5,
    filter: "blur(3px)",
  },
  open: {
    opacity: 1,
    scaleY: 1,
    y: 0,
    filter: "blur(0px)",
  },
};

export const REDUCED_DROPDOWN_SURFACE_VARIANTS: Variants = {
  closed: { opacity: 1, scaleY: 1, y: 0, filter: "blur(0px)" },
  open: { opacity: 1, scaleY: 1, y: 0, filter: "blur(0px)" },
};

// Radix measures Select content while positioning it. Keep this surface
// geometry-stable so animation cannot shift the Popper anchor.
export const SELECT_SURFACE_VARIANTS: Variants = {
  closed: { opacity: 0 },
  open: { opacity: 1 },
};

export const REDUCED_SELECT_SURFACE_VARIANTS: Variants = {
  closed: { opacity: 1 },
  open: { opacity: 1 },
};

export const SELECT_SURFACE_TRANSITION: Transition = {
  duration: 0.18,
  ease: [0.16, 1, 0.3, 1],
};

export const DROPDOWN_SURFACE_TRANSITION: Transition = {
  opacity: { duration: 0.18 },
  scaleY: { type: "spring", duration: 0.42, bounce: 0.14 },
  y: { type: "spring", duration: 0.42, bounce: 0.14 },
  filter: { duration: 0.18 },
};

type AnimatedDropdownSurfaceProps = Omit<
  HTMLMotionProps<"div">,
  "animate" | "children" | "initial" | "transition" | "variants"
> & {
  children: ReactNode;
  kind: "combobox" | "select";
  open: boolean;
};

export function AnimatedDropdownSurface({
  children,
  kind,
  open,
  style,
  ...props
}: AnimatedDropdownSurfaceProps) {
  const reduce = useReducedMotion() ?? false;
  if (!open) return null;

  return (
    <motion.div
      {...props}
      data-motion-dropdown={kind}
      data-motion-reduced={reduce || undefined}
      initial={reduce ? false : "closed"}
      animate="open"
      variants={reduce ? REDUCED_DROPDOWN_SURFACE_VARIANTS : DROPDOWN_SURFACE_VARIANTS}
      transition={reduce ? { duration: 0 } : DROPDOWN_SURFACE_TRANSITION}
      style={{
        ...style,
        transformOrigin: style?.transformOrigin ?? "top",
      }}
    >
      {children}
    </motion.div>
  );
}

export function useDropdownReducedMotion() {
  return useReducedMotion() ?? false;
}
