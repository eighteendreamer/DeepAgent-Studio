// One motion source for the reference demo and the production UI.
export const SHARE_MOTION = {
  open: {
    width: { stiffness: 306.25, damping: 28, duration: 0.7, delay: 0 },
    height: { stiffness: 196, damping: 18.34, duration: 0.86, delay: 0.057 },
  },
  close: {
    width: { stiffness: 400, damping: 33, duration: 0.64, delay: 0.07 },
    height: { stiffness: 600.25, damping: 40, duration: 0.62, delay: 0 },
  },
  resize: { stiffness: 280, damping: 24, duration: 0.64 },
} as const;

export function springEase(stiffness: number, damping: number, duration: number) {
  const decay = damping / 2;
  const frequency = Math.sqrt(stiffness - decay * decay);
  return (progress: number) => {
    if (progress === 0 || progress === 1) return progress;
    const time = progress * duration;
    return 1 - Math.exp(-decay * time) *
      (Math.cos(frequency * time) + decay / frequency * Math.sin(frequency * time));
  };
}

export function prefersReducedMotion() {
  return typeof window !== "undefined" && window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

/** GSAP's color parser expects RGB/HSL; themes may resolve to OKLab/color(). */
export function motionColor(color: string): string {
  if (/^rgba?\(/i.test(color)) return color;
  const canvas = document.createElement("canvas");
  canvas.width = canvas.height = 1;
  const context = canvas.getContext("2d");
  if (!context) return color;
  context.fillStyle = color;
  context.fillRect(0, 0, 1, 1);
  const [red, green, blue, alpha] = context.getImageData(0, 0, 1, 1).data;
  return `rgba(${red}, ${green}, ${blue}, ${alpha / 255})`;
}

export const SOFT_SPRING = { stiffness: 360, damping: 34, duration: 0.42 } as const;
