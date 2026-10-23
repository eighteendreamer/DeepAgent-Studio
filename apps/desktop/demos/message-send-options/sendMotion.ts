import gsap from "gsap";
import { continuousRadius } from "../../src/theme/continuousCorners";
import { prefersReducedMotion, springEase } from "../../src/components/ui/springMotion";

import { playMessageSendFlight } from "../../src/components/ui/messageSendFlight";

export type Variant = "A" | "B" | "C" | "D" | "E" | "F";
export type MotionSettings = { duration: number; bounce: number; arc: number };
export type MotionProfile = "classic" | "selector";
type Glyph = { text: string; x: number; y: number };
type Box = { x: number; y: number; width: number; height: number };
export type Flight = { finish: () => void; cancel: () => void };

export function graphemes(text: string): string[] {
  return typeof Intl.Segmenter === "function"
    ? Array.from(new Intl.Segmenter("zh-CN", { granularity: "grapheme" }).segment(text), item => item.segment)
    : Array.from(text);
}

const mix = (a: number, b: number, p: number) => a + (b - a) * p;
const clamp = (p: number) => Math.max(0, Math.min(1, p));
function point(start: Box, end: Box, progress: number, arc: number) {
  const sx = start.x + start.width / 2, sy = start.y + start.height / 2;
  const ex = end.x + end.width / 2, ey = end.y + end.height / 2;
  const cx = (sx + ex) / 2 + arc, cy = (sy + ey) / 2 - Math.abs(arc) * .5;
  const q = 1 - progress;
  return { x: q*q*sx + 2*q*progress*cx + progress*progress*ex,
    y: q*q*sy + 2*q*progress*cy + progress*progress*ey };
}
function svgPath(layer: HTMLElement, className: string) {
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("class", "send-vector");
  svg.setAttribute("width", String(layer.clientWidth));
  svg.setAttribute("height", String(layer.clientHeight));
  const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
  path.setAttribute("class", className);
  svg.append(path); layer.append(svg);
  return path;
}

/** Measured source → measured destination; the real bubble stays hidden until handoff. */
export function sendMotion({ variant, profile = "classic", layer, composer, start, end, target, text, sourceGlyphs, settings, slow, done }: {
  variant: Variant; layer: HTMLElement; composer: HTMLElement; start: Box; end: Box;
  target: HTMLElement; text: string; sourceGlyphs: Glyph[]; settings: MotionSettings;
  profile?: MotionProfile; slow: boolean; done: () => void;
}): Flight {
  if (prefersReducedMotion()) { done(); return { finish() {}, cancel() {} }; }
  if (variant === "F" && profile === "selector") {
    const carrier = document.createElement("div");
    carrier.className = "send-carrier"; carrier.textContent = text; layer.append(carrier);
    const flight = playMessageSendFlight({ carrier, composer, start, getEnd: () => end,
      boundsWidth: layer.clientWidth,
      appearance: { end: { fontSize: 13, lineHeight: 20,
        radii: Array(4).fill(continuousRadius(14)) as number[], background: "rgb(239,241,244)" } },
      onFinish: () => { layer.replaceChildren(); done(); },
    });
    if (slow) flight.timeline.timeScale(.55);
    return flight;
  }
  let completed = false;
  const finish = () => {
    if (completed) return;
    completed = true;
    layer.replaceChildren();
    gsap.set(composer, { clearProps: "transform" });
    done();
  };
  const timeline = gsap.timeline({ onComplete: finish });
  if (slow) timeline.timeScale(.55);
  const duration = variant === "F" ? settings.duration : { A: .64, B: .82, C: .7, D: .8, E: .86 }[variant];
  timeline.to(composer, { scaleY: variant === "F" ? .94 : .985, y: variant === "F" ? 3 : 1, duration: .09, transformOrigin: "50% 100%" }, 0);
  timeline.to(composer, { scaleY: 1, y: 0, duration: .4, ease: springEase(360, 26, .4) }, .09);

  if (variant === "C") {
    const skin = document.createElement("div");
    skin.className = "send-carrier letter-skin";
    layer.append(skin);
    gsap.set(skin, { x: end.x, y: end.y, width: end.width, height: end.height, scale: .9, opacity: 0, transformOrigin: "50% 100%", borderRadius: continuousRadius(14) });
    timeline.to(skin, { scale: 1, opacity: 1, duration: .4, ease: "power2.out" }, .15);
    const origin = layer.getBoundingClientRect();
    const destinations = Array.from(target.querySelectorAll<HTMLElement>("[data-character]"));
    const step = Math.min(.018, .18 / Math.max(1, destinations.length));
    destinations.forEach((character, index) => {
      const rect = character.getBoundingClientRect();
      const source = sourceGlyphs[index] ?? sourceGlyphs[sourceGlyphs.length - 1];
      if (!source) return;
      const glyph = document.createElement("span");
      glyph.className = "send-character";
      glyph.textContent = character.textContent;
      layer.append(glyph);
      gsap.set(glyph, { x: source.x, y: source.y });
      timeline.to(glyph, { x: rect.left - origin.left, y: rect.top - origin.top,
        duration: .52, ease: "power3.inOut" }, index * step);
    });
    timeline.to({}, { duration: .06 });
  } else {
    const arc = variant === "D" ? 40 : variant === "E" ? -38 : variant === "F" ? settings.arc : variant === "B" ? -12 : 0;
    let tether: SVGPathElement | undefined;
    let route: SVGPathElement | undefined;
    if (variant === "B") tether = svgPath(layer, "send-tether");
    if (variant === "E") {
      route = svgPath(layer, "send-route");
      const first = point(start, end, 0, arc), last = point(start, end, 1, arc);
      const cx = (first.x + last.x) / 2 + arc, cy = (first.y + last.y) / 2 - Math.abs(arc) * .5;
      route.setAttribute("d", `M${first.x},${first.y} Q${cx},${cy} ${last.x},${last.y}`);
      const length = route.getTotalLength();
      gsap.set(route, { strokeDasharray: length, strokeDashoffset: length, opacity: .8 });
      timeline.to(route, { strokeDashoffset: 0, duration: duration * .82, ease: "power2.inOut" }, .02);
      timeline.to(route, { opacity: 0, duration: .22 }, duration * .78);
    }
    const carrier = document.createElement("div");
    carrier.className = "send-carrier";
    carrier.textContent = text;
    layer.append(carrier);
    const progress = { value: 0, squash: 1 };
    const paint = () => {
      const p = progress.value, bounded = clamp(p);
      const center = point(start, end, p, arc);
      const width = mix(start.width, end.width, bounded), height = mix(start.height, end.height, bounded);
      const rotation = variant === "D" ? -7 * Math.sin(Math.PI * bounded) : 0;
      const x = Math.max(3, Math.min(center.x - width/2, layer.clientWidth - width - 3));
      gsap.set(carrier, { x, y: center.y - height/2, width, height, rotation,
        scaleX: variant === "F" ? 1 + (1-progress.squash)*.2 + .055 * Math.sin(Math.PI * p * 2) : 1,
        scaleY: variant === "F" ? progress.squash * (1 - .055 * Math.sin(Math.PI * p * 2)) : 1,
        borderRadius: mix(continuousRadius(9), continuousRadius(14), bounded),
        backgroundColor: `rgba(239,241,244,${variant === "B" ? 1 : clamp(bounded * 5)})`,
        boxShadow: variant === "D" ? `0 ${Math.sin(Math.PI * bounded) * 7}px ${Math.sin(Math.PI * bounded) * 18}px rgba(30,40,50,${Math.sin(Math.PI * bounded) * .08})` : "none" });
      if (tether) {
        const sx = start.x + start.width / 2, sy = start.y + start.height * .7;
        const tx = center.x, ty = center.y + height * .32;
        const dx = tx-sx, dy = ty-sy, distance = Math.max(1,Math.hypot(dx,dy));
        const ux = dx/distance, uy = dy/distance, nx = -uy, ny = ux;
        const neck = Math.sqrt(Math.max(0, 1 - bounded / .86)), r0 = 13 * neck, r1 = 14 * neck;
        const at = (x: number,y: number,r: number) => `${x + nx*r},${y + ny*r}`;
        if (bounded < .86) {
          tether.setAttribute("d", `M${at(sx,sy,-r0)} C${at(sx+dx*.38,sy+dy*.38,-neck*3)} ${at(tx-dx*.32,ty-dy*.32,-neck*3)} ${at(tx,ty,-r1)} Q${tx+ux*r1},${ty+uy*r1} ${at(tx,ty,r1)} C${at(tx-dx*.32,ty-dy*.32,neck*3)} ${at(sx+dx*.38,sy+dy*.38,neck*3)} ${at(sx,sy,r0)} Q${sx-ux*r0},${sy-uy*r0} ${at(sx,sy,-r0)}Z`);
          tether.style.opacity = String(clamp((.86-bounded)/.12));
        } else tether.style.opacity = "0";
      }
    };
    paint();
    if (variant === "F") {
      timeline.to(progress, { squash: .82, duration: .13, ease: "power2.in", onUpdate: paint }, 0);
      timeline.to(progress, { squash: 1, duration: .28, ease: "power2.out", onUpdate: paint }, .13);
    }
    if (variant === "B") {
      timeline.to(progress, { value: .86, duration: duration * .73, ease: "power2.inOut", onUpdate: paint }, .015);
      timeline.to(progress, { value: 1, duration: duration * .42,
        ease: springEase(300, 22, duration * .42), onUpdate: paint }, duration * .73 + .015);
    } else {
      const ease = variant === "F" ? springEase(360, 32 - settings.bounce * 14, duration) : "power3.inOut";
      timeline.to(progress, { value: 1, duration, ease, onUpdate: paint }, variant === "F" ? .13 : .015);
    }
  }
  return {
    finish: () => { timeline.progress(1); finish(); },
    cancel: () => { timeline.kill(); layer.replaceChildren(); gsap.set(composer, { clearProps: "transform" }); },
  };
}
