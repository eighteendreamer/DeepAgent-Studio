/** The working/default sphere from the supplied Thinking Orbs reference.
 * Each mounted, visible active orb owns one clock; settled history has none. */
export function mountWorkingOrb(svg: SVGSVGElement, active: boolean): () => void {
  const size = 20;
  const golden = Math.PI * (3 - Math.sqrt(5));
  const dots = Array.from({ length: 80 }, (_, i) => {
    const y = 1 - (i + 0.5) / 80 * 2;
    const radius = Math.sqrt(1 - y * y);
    const circle = document.createElementNS("http://www.w3.org/2000/svg", "circle");
    circle.setAttribute("fill", "currentColor");
    svg.appendChild(circle);
    return { point: [Math.cos(golden * i) * radius, y, Math.sin(golden * i) * radius], circle };
  });
  const draw = (time: number) => {
    const yaw = time / 3000 * Math.PI * 2;
    const tilt = 20 * Math.PI / 180;
    const tip = 30 * Math.PI / 180, roll = 10 * Math.PI / 180;
    const axis = [-Math.sin(roll) * Math.cos(tip), Math.cos(roll) * Math.cos(tip), Math.sin(tip)];
    const u = time % 1700;
    const ease = (1 - Math.cos(Math.PI * Math.min(1, u / 1200))) / 2;
    const at = 1.3 - 2.6 * ease;
    for (const { point: [x, y, z], circle } of dots) {
      let vx = x * Math.cos(yaw) + z * Math.sin(yaw);
      const rz = z * Math.cos(yaw) - x * Math.sin(yaw);
      let vy = y * Math.cos(tilt) - rz * Math.sin(tilt);
      const vz = y * Math.sin(tilt) + rz * Math.cos(tilt);
      const d = (vz + 1) / 2;
      let r = (.5 + 1.4 * d) * Math.pow(size / 64, .6) * .72;
      let opacity = Math.max(0, (d - .3) / .7);
      const q = vx * axis[0] + vy * axis[1] + vz * axis[2];
      const glow = u < 1200 ? Math.exp(-Math.pow((q - at) / .2, 2)) : 0;
      r *= 1 + .6 * glow;
      opacity += (1 - opacity) * glow;
      const wave = Math.min(1, Math.max(0, (q - at) / .2)) *
        (u < 1200 ? 1 : 1 - Math.min(1, 1.6 * ((u - 1200) / 800)));
      vx *= 1 - .08 * wave; vy *= 1 - .08 * wave; r *= 1 - .15 * wave;
      circle.setAttribute("cx", String(10 + vx * 8));
      circle.setAttribute("cy", String(10 - vy * 8));
      circle.setAttribute("r", String(Math.max(.45, r)));
      circle.setAttribute("fill-opacity", String(opacity));
    }
  };
  let frame = 0, last = 0, time = 750, visible = true, disposed = false;
  const reduced = window.matchMedia("(prefers-reduced-motion: reduce)");
  const canAnimate = () => active && visible && !document.hidden && !reduced.matches && !disposed;
  const tick = (now: number) => {
    frame = 0;
    if (!canAnimate()) return;
    time += last ? Math.min(100, now - last) : 0;
    last = now;
    draw(time);
    frame = requestAnimationFrame(tick);
  };
  const sync = () => {
    if (frame) cancelAnimationFrame(frame);
    frame = 0; last = 0;
    if (canAnimate()) frame = requestAnimationFrame(tick);
  };
  const observer = new IntersectionObserver(entries => {
    visible = entries.some(entry => entry.isIntersecting);
    sync();
  });
  draw(time);
  observer.observe(svg);
  document.addEventListener("visibilitychange", sync);
  reduced.addEventListener("change", sync);
  sync();
  return () => {
    disposed = true;
    if (frame) cancelAnimationFrame(frame);
    observer.disconnect();
    reduced.removeEventListener("change", sync);
    document.removeEventListener("visibilitychange", sync);
    dots.forEach(({ circle }) => circle.remove());
  };
}
