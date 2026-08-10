import gsap from "gsap";

/** 方案 A：从触发按钮中心 scale 展开 / 收回 */
export const MODAL_ORIGIN_MOTION = {
  openBackdropDuration: 0.38,
  openPanelDuration: 0.52,
  openPanelDelay: 0.02,
  closePanelDuration: 0.42,
  closeBackdropDuration: 0.3,
  closeBackdropDelay: 0.1,
  scaleFrom: 0.04,
  openPanelEase: "power4.inOut",
  openBackdropEase: "power2.out",
  closePanelEase: "power4.inOut",
  closeBackdropEase: "power2.in",
} as const;

export interface ModalTriggerOrigin {
  centerX: number;
  centerY: number;
}

export function prefersReducedMotion(): boolean {
  return typeof window !== "undefined" && window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

export function originTransformFromPanel(panel: HTMLElement, origin: ModalTriggerOrigin): string {
  const rect = panel.getBoundingClientRect();
  const originX = origin.centerX - rect.left;
  const originY = origin.centerY - rect.top;
  return `${originX}px ${originY}px`;
}

export function modalOriginFromElement(element: HTMLElement): ModalTriggerOrigin {
  const rect = element.getBoundingClientRect();
  return {
    centerX: rect.left + rect.width / 2,
    centerY: rect.top + rect.height / 2,
  };
}

export function playModalOriginOpen(
  backdrop: HTMLElement,
  panel: HTMLElement,
  origin: ModalTriggerOrigin | null,
): gsap.core.Timeline {
  const tl = gsap.timeline();

  if (prefersReducedMotion() || !origin) {
    gsap.set(backdrop, { autoAlpha: 1 });
    gsap.set(panel, { autoAlpha: 1, scale: 1, clearProps: "transform,transformOrigin" });
    return tl;
  }

  const transformOrigin = originTransformFromPanel(panel, origin);
  gsap.set(backdrop, { autoAlpha: 0 });
  gsap.set(panel, {
    autoAlpha: 0,
    scale: MODAL_ORIGIN_MOTION.scaleFrom,
    transformOrigin,
  });

  tl.to(backdrop, {
    autoAlpha: 1,
    duration: MODAL_ORIGIN_MOTION.openBackdropDuration,
    ease: MODAL_ORIGIN_MOTION.openBackdropEase,
  }, 0).to(
    panel,
    {
      autoAlpha: 1,
      scale: 1,
      duration: MODAL_ORIGIN_MOTION.openPanelDuration,
      ease: MODAL_ORIGIN_MOTION.openPanelEase,
    },
    MODAL_ORIGIN_MOTION.openPanelDelay,
  );

  return tl;
}

export function playModalOriginClose(
  backdrop: HTMLElement,
  panel: HTMLElement,
  origin: ModalTriggerOrigin | null,
): gsap.core.Timeline {
  const tl = gsap.timeline();

  if (prefersReducedMotion() || !origin) {
    gsap.set([backdrop, panel], { autoAlpha: 0 });
    return tl;
  }

  const transformOrigin = originTransformFromPanel(panel, origin);
  gsap.set(panel, { transformOrigin });

  tl.to(
    panel,
    {
      autoAlpha: 0,
      scale: MODAL_ORIGIN_MOTION.scaleFrom,
      duration: MODAL_ORIGIN_MOTION.closePanelDuration,
      ease: MODAL_ORIGIN_MOTION.closePanelEase,
    },
    0,
  ).to(
    backdrop,
    {
      autoAlpha: 0,
      duration: MODAL_ORIGIN_MOTION.closeBackdropDuration,
      ease: MODAL_ORIGIN_MOTION.closeBackdropEase,
    },
    MODAL_ORIGIN_MOTION.closeBackdropDelay,
  );

  return tl;
}
