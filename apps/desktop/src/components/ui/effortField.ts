/** Canvas field recreated from the public Hexta Models preview, without its Pro source. */
export function createEffortField(canvas: HTMLCanvasElement, getRatio: () => number) {
  const ctx = canvas.getContext("2d");
  let width = 0;
  let height = 0;
  let phase = 0;
  let lastTime = 0;
  const stars = Array.from({ length: 64 }, (_, i) => ({
    x: ((i * 0.61803398875) % 1),
    y: ((i * 0.754877666) % 1),
    size: 0.35 + (i % 5) * 0.13,
    speed: 0.3 + (i % 7) * 0.1,
  }));

  const resize = () => {
    // The enclosing model menu morphs with a CSS transform; use layout pixels.
    width = canvas.clientWidth;
    height = canvas.clientHeight;
    const dpr = Math.min(window.devicePixelRatio || 1, 2);
    canvas.width = Math.max(1, Math.round(width * dpr));
    canvas.height = Math.max(1, Math.round(height * dpr));
    ctx?.setTransform(dpr, 0, 0, dpr, 0, 0);
  };

  const draw = (time = 0, still = false, motionBoost = 1) => {
    if (!ctx || width <= 0 || height <= 0) return;
    const ratio = getRatio();
    // Integrate speed so changing effort never jumps the particles' positions.
    if (!still) phase += Math.min(Math.max(time - lastTime, 0), 0.05)
      * (0.16 + ratio * 0.85) * Math.max(1, motionBoost);
    lastTime = time;
    const hue = 210 + ratio * ratio * 63;
    const fill = 32 + (width - 32) * ratio;
    ctx.clearRect(0, 0, width, height);
    const base = ctx.createLinearGradient(0, 0, Math.max(fill, 32), 0);
    base.addColorStop(0, "#30343a");
    base.addColorStop(0.28, `hsl(${hue - 12} 38% 47%)`);
    base.addColorStop(0.65, `hsl(${hue} 79% 70%)`);
    base.addColorStop(1, `hsl(${hue + 5} 88% 70%)`);
    ctx.fillStyle = base;
    ctx.fillRect(0, 0, width, height);

    ctx.save();
    ctx.globalCompositeOperation = "screen";
    // Soft overlapping ribbons create the drifting, diagonal nebula texture.
    for (let band = 0; band < 7; band++) {
      ctx.beginPath();
      for (let x = 0; x <= width + 4; x += 4) {
        const y = height * (0.12 + band * 0.15)
          + Math.sin(x * 0.034 + phase + band * 1.8) * 5
          + Math.sin(x * 0.079 - phase * 0.6 + band) * 2 - x * 0.035;
        if (x === 0) ctx.moveTo(x, y);
        else ctx.lineTo(x, y);
      }
      ctx.lineWidth = 2 + (band % 3) * 2;
      ctx.strokeStyle = `hsla(${hue + band * 2} 90% 87% / ${0.06 + ratio * 0.025})`;
      ctx.stroke();
    }
    const glowX = fill * (0.6 + Math.sin(phase * 0.7) * 0.12);
    const glow = ctx.createRadialGradient(glowX, height * 0.7, 0, glowX, height * 0.7, 46);
    glow.addColorStop(0, `hsla(${hue - 12} 100% 88% / .35)`);
    glow.addColorStop(1, `hsla(${hue} 100% 80% / 0)`);
    ctx.fillStyle = glow;
    ctx.fillRect(0, 0, width, height);

    for (const [i, star] of stars.entries()) {
      const x = ((star.x + phase * star.speed * 0.08) % 1) * width;
      const y = ((star.y - phase * star.speed * 0.04) % 1 + 1) % 1 * height;
      const fade = Math.min(1, x / 55);
      const twinkle = 0.45 + Math.sin(phase * 2 + i * 1.9) * 0.25;
      ctx.fillStyle = `rgba(255,255,255,${fade * twinkle * (0.4 + ratio * 0.6)})`;
      ctx.beginPath();
      ctx.ellipse(x, y, star.size * (1 + ratio * 0.5), star.size * 0.65, -0.45, 0, Math.PI * 2);
      ctx.fill();
    }
    ctx.restore();
    const shade = ctx.createLinearGradient(0, 0, Math.max(fill, 32), 0);
    shade.addColorStop(0, "rgba(36,39,44,.8)");
    shade.addColorStop(0.45, "rgba(36,39,44,0)");
    ctx.fillStyle = shade;
    ctx.fillRect(0, 0, width, height);
  };

  const observer = new ResizeObserver(() => { resize(); draw(lastTime, true); });
  observer.observe(canvas);
  resize();
  return { draw, dispose: () => observer.disconnect() };
}
