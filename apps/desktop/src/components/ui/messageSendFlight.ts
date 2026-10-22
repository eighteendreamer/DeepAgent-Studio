import gsap from "gsap";
import { continuousRadius } from "../../theme/continuousCorners";
import { SHARE_MOTION, springEase } from "./springMotion";

export type SendBox = { x: number; y: number; width: number; height: number };
export type SendFlight = { finish: () => void; cancel: () => void };
type Appearance = { fontSize: number; lineHeight: number; radii: number[]; background: string };
const mix = (a: number, b: number, p: number) => a + (b - a) * p;
const clamp = (p: number) => Math.max(0, Math.min(1, p));

/** The approved F preview: gentle preload, the model menu's two spring axes,
 * and one small arrival overshoot. Preview and production share this timeline. */
export function playMessageSendFlight({
  carrier, composer, start, getEnd, appearance, boundsWidth = window.innerWidth, onFinish,
}: {
  carrier: HTMLElement;
  composer: HTMLElement;
  start: SendBox;
  getEnd: () => SendBox | null;
  appearance: { start?: Partial<Appearance>; end: Appearance };
  boundsWidth?: number;
  onFinish: () => void;
}): SendFlight & { timeline: gsap.core.Timeline } {
  const progress = { width: 0, height: 0, squash: 1 };
  const axes = SHARE_MOTION.open;
  const from = { fontSize: appearance.end.fontSize, lineHeight: appearance.end.lineHeight,
    radii: Array(4).fill(continuousRadius(9)) as number[], ...appearance.start };
  const background = gsap.utils.interpolate("rgba(239,241,244,0)", appearance.end.background);
  const originalTransition = composer.style.transition;
  const originalTransform = composer.style.transform;
  const originalOrigin = composer.style.transformOrigin;
  let finished = false;
  const finish = () => {
    if (finished) return;
    finished = true;
    timeline.kill();
    gsap.set(composer, { clearProps: "transform,transformOrigin" });
    composer.style.transform = originalTransform;
    composer.style.transformOrigin = originalOrigin;
    composer.style.transition = originalTransition;
    onFinish();
  };
  const paint = () => {
    const end = getEnd();
    if (!end) { finish(); return; }
    const p = progress.height, bounded = clamp(p), widthP = progress.width;
    const width = mix(start.width, end.width, clamp(widthP));
    const height = mix(start.height, end.height, bounded);
    const centerX = mix(start.x + start.width/2, end.x + end.width/2, widthP) + 8 * Math.sin(Math.PI * bounded);
    const centerY = mix(start.y + start.height/2, end.y + end.height/2, p);
    // Compress the settled layout rather than rewrapping its last character
    // into a clipped extra line when the spring passes the final width.
    const springX = Math.max(24, mix(start.width, end.width, widthP)) / width;
    const springY = Math.max(24, mix(start.height, end.height, p)) / height;
    const lag = Math.max(-.15, Math.min(.4, widthP - p));
    const arrival = Math.max(0, p - 1);
    const radii = from.radii.map((radius, index) => mix(radius, appearance.end.radii[index], bounded));
    gsap.set(carrier, {
      x: Math.max(3, Math.min(centerX - width/2, boundsWidth - width - 3)),
      y: centerY - height/2, width, height,
      scaleX: springX * (1 + (1-progress.squash)*.15 - lag*.035 + arrival*.45),
      scaleY: springY * progress.squash * (1 + lag*.07 - arrival*.3),
      borderRadius: radii.map(radius => `${radius}px`).join(" "),
      backgroundColor: background(clamp(widthP * 5)),
      fontSize: mix(from.fontSize, appearance.end.fontSize, bounded),
      lineHeight: `${mix(from.lineHeight, appearance.end.lineHeight, bounded)}px`,
    });
  };
  const timeline = gsap.timeline({ onComplete: finish });
  composer.style.transition = "none";
  paint();
  timeline.to(composer, { scaleY: .975, y: 1.5, duration: .09, transformOrigin: "50% 100%" }, 0);
  timeline.to(composer, { scaleY: 1, y: 0, duration: axes.height.duration,
    ease: springEase(axes.height.stiffness, axes.height.damping, axes.height.duration) }, .09);
  timeline.to(progress, { squash: .94, duration: .09, ease: "power2.in", onUpdate: paint }, 0);
  timeline.to(progress, { squash: 1, duration: .22, ease: "power2.out", onUpdate: paint }, .09);
  for (const axis of ["width", "height"] as const) {
    const motion = axes[axis];
    timeline.to(progress, { [axis]: 1, duration: motion.duration,
      ease: springEase(motion.stiffness, motion.damping, motion.duration), onUpdate: paint }, .08 + motion.delay);
  }
  return { timeline, finish: () => { timeline.progress(1); finish(); }, cancel: finish };
}
