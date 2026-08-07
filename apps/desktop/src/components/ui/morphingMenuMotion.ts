import type { Transition, Variants } from "framer-motion";

/** Morphing Select / 二级 flyout 共享 spring */
export function morphSpringTransition(reduced: boolean | null, duration = 0.62): Transition {
  if (reduced) return { duration: 0 };
  return {
    type: "spring",
    stiffness: 250,
    damping: 20,
    mass: 1,
    duration,
  };
}

export function morphListVariants(reduced: boolean | null, stagger = 0.02): Variants {
  return {
    hidden: { opacity: 0 },
    visible: {
      opacity: 1,
      transition: {
        delayChildren: reduced ? 0 : 0.1,
        staggerChildren: reduced ? 0 : stagger,
      },
    },
  };
}

export function morphItemVariants(reduced: boolean | null): Variants {
  return {
    hidden: {
      opacity: 0,
      y: 10,
      filter: reduced ? "blur(0px)" : "blur(4px)",
    },
    visible: {
      opacity: 1,
      y: 0,
      filter: "blur(0px)",
      transition: reduced
        ? { duration: 0 }
        : { type: "spring", stiffness: 300, damping: 24 },
    },
  };
}
