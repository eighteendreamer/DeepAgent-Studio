import { gsap } from "gsap";
import { createEffortField } from "./effortField";
import { springEase } from "./springMotion";

// Position keeps A's measured, non-overshooting 500 ms glide.
const SPRING = [[0, 0], [.02, .015], [.04, .0532], [.06, .1065], [.08, .1686],
  [.1, .2351], [.13, .3363], [.16, .4332], [.2, .5495], [.24, .648], [.28, .7287],
  [.33, .807], [.38, .8646], [.44, .9128], [.5, .9447], [.57, .968], [.65, .9834],
  [.74, .9924], [.85, .9975], [1, 1]];
function glideEase(progress: number) {
  const end = SPRING.findIndex(([x]) => x >= progress);
  if (end <= 0) return end === 0 ? 0 : 1;
  const [x0, y0] = SPRING[end - 1];
  const [x1, y1] = SPRING[end];
  return y0 + (y1 - y0) * (progress - x0) / (x1 - x0);
}

// Local design: material deformation springs back while the selected stop stays fixed.
const returnEase = springEase(400, 23, 0.72);

/** One motion implementation shared by the desktop component and its review file. */
export function createEffortMotion(
  root: HTMLElement,
  canvas: HTMLCanvasElement,
  initialRatio: number,
  initialReducedMotion: boolean,
) {
  const state = { ratio: initialRatio, press: 1, body: 0, lean: 0, boost: 1 };
  let targetRatio = initialRatio;
  let reduced = initialReducedMotion;
  let attached = false;
  let disposed = false;
  let positionTween: gsap.core.Tween | undefined;
  let pressTween: gsap.core.Tween | undefined;
  let shapeTween: gsap.core.Timeline | gsap.core.Tween | undefined;
  let boostTween: gsap.core.Tween | undefined;
  const field = createEffortField(canvas, () => state.ratio);
  const variables = () => ({
    "--effort-ratio": String(state.ratio),
    "--effort-press": String(state.press),
    "--effort-jelly-x": String(1 + state.body),
    "--effort-jelly-y": String(1 - state.body * 0.3),
    "--effort-jelly-offset": `${-state.lean * 9}px`,
    "--effort-jelly-skew": `${-state.lean * 12}deg`,
    "--effort-jelly-left": `${50 + state.lean * 32}%`,
    "--effort-jelly-right": `${50 - state.lean * 24}%`,
  });
  const context = gsap.context(() => { gsap.set(root, variables()); }, root);
  const paint = () => {
    if (disposed) return;
    for (const [name, value] of Object.entries(variables())) root.style.setProperty(name, value);
    if (reduced) field.draw(gsap.ticker.time, true);
  };
  const draw = (time: number) => {
    if (!document.hidden) field.draw(time, false, state.boost);
  };
  const syncTicker = () => {
    if (attached) gsap.ticker.remove(draw);
    attached = !reduced;
    field.draw(gsap.ticker.time, true);
    if (attached) gsap.ticker.add(draw);
  };
  const stopTweens = () => {
    positionTween?.kill();
    pressTween?.kill();
    shapeTween?.kill();
    boostTween?.kill();
  };
  syncTicker();

  return {
    moveTo(ratio: number, immediate: boolean) {
      targetRatio = Math.max(0, Math.min(1, ratio));
      positionTween?.kill();
      positionTween = gsap.to(state, {
        ratio: targetRatio, duration: immediate || reduced ? 0 : 0.5,
        ease: glideEase, onUpdate: paint,
      });
    },
    press() {
      pressTween?.kill();
      pressTween = gsap.to(state, { press: reduced ? 1 : 0.96, duration: reduced ? 0 : 0.08, onUpdate: paint });
    },
    drag(velocity: number) {
      if (reduced || disposed) return;
      const speed = Math.min(3.5, Math.abs(velocity));
      const deformation = Math.min(0.46, 0.045 + speed * 0.17);
      const lean = Math.sign(velocity) * deformation;
      shapeTween?.kill();
      // On reversal, lean crosses through its previous value instead of flipping instantly.
      // When the pointer pauses, the same material relaxes without waiting for pointerup.
      shapeTween = gsap.timeline({ onUpdate: paint })
        .to(state, { body: deformation, lean, duration: 0.09, ease: "power2.out" })
        .to(state, { body: 0, lean: 0, duration: 0.72, ease: returnEase }, ">0.07");
      boostTween?.kill();
      boostTween = gsap.to(state, {
        boost: 1 + Math.min(2.2, 0.55 + speed * 0.6), duration: 0.12,
        ease: "power2.out",
      });
    },
    release() {
      pressTween?.kill();
      shapeTween?.kill();
      boostTween?.kill();
      pressTween = gsap.to(state, { press: 1, duration: reduced ? 0 : 0.32, ease: "power2.out", onUpdate: paint });
      shapeTween = gsap.to(state, {
        body: 0, lean: 0, duration: reduced ? 0 : 0.72, ease: returnEase, onUpdate: paint,
      });
      boostTween = gsap.to(state, { boost: 1, duration: reduced ? 0 : 0.7, ease: "power2.out" });
    },
    setReducedMotion(next: boolean) {
      if (next === reduced) return;
      reduced = next;
      stopTweens();
      Object.assign(state, { ratio: targetRatio, press: 1, body: 0, lean: 0, boost: 1 });
      paint();
      syncTicker();
    },
    dispose() {
      disposed = true;
      stopTweens();
      gsap.ticker.remove(draw);
      field.dispose();
      context.revert();
    },
  };
}
