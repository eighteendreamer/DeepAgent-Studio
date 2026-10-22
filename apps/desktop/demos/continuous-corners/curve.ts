/** @license
 * Equal-radius subset adapted from figma-squircle's draw.ts, with preserveSmoothing.
 * Source: https://github.com/phamfoo/figma-squircle/blob/main/src/draw.ts
 * This is a web approximation, not Apple's native CALayer implementation.
 *
 * MIT License
 * Copyright (c) 2021 Tien Pham
 * Permission is hereby granted, free of charge, to any person obtaining a copy
 * of this software and associated documentation files (the "Software"), to deal
 * in the Software without restriction, including without limitation the rights
 * to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
 * copies of the Software, and to permit persons to whom the Software is
 * furnished to do so, subject to the following conditions:
 * The above copyright notice and this permission notice shall be included in all
 * copies or substantial portions of the Software.
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
 * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
 * AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
 * LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
 * OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
 * SOFTWARE.
 */
export function cornerPath(width: number, height: number, radius: number, smoothing: number) {
  if (width <= 0 || height <= 0) return "";
  const budget = Math.min(width, height) / 2;
  const r = Math.max(0, Math.min(radius, budget));
  if (r === 0) return `M 0 0 H ${width} V ${height} H 0 Z`;
  const s = Math.max(0, Math.min(smoothing, 1));
  const radians = (degrees: number) => degrees * Math.PI / 180;
  let p = (1 + s) * r;
  const arc = Math.sin(radians(90 * (1 - s) / 2)) * r * Math.SQRT2;
  const distance = r * Math.tan(radians(45 * s / 2));
  const c = distance * Math.cos(radians(45 * s));
  const d = c * Math.tan(radians(45 * s));
  let b = (p - arc - c - d) / 3;
  let a = 2 * b;
  if (p > budget) {
    const available = budget - d - arc - c;
    b = Math.min(b, available * 5 / 6);
    a = available - b;
    p = budget;
  }
  // Bézier transitions have zero curvature at the straight-edge junction.
  // An SVG background keeps child focus rings and future popovers unclipped.
  return `M ${width - p} 0
    c ${a} 0 ${a + b} 0 ${a + b + c} ${d}
    a ${r} ${r} 0 0 1 ${arc} ${arc}
    c ${d} ${c} ${d} ${b + c} ${d} ${a + b + c}
    L ${width} ${height - p}
    c 0 ${a} 0 ${a + b} ${-d} ${a + b + c}
    a ${r} ${r} 0 0 1 ${-arc} ${arc}
    c ${-c} ${d} ${-(b + c)} ${d} ${-(a + b + c)} ${d}
    L ${p} ${height}
    c ${-a} 0 ${-(a + b)} 0 ${-(a + b + c)} ${-d}
    a ${r} ${r} 0 0 1 ${-arc} ${-arc}
    c ${-d} ${-c} ${-d} ${-(b + c)} ${-d} ${-(a + b + c)}
    L 0 ${p}
    c 0 ${-a} 0 ${-(a + b)} ${d} ${-(a + b + c)}
    a ${r} ${r} 0 0 1 ${arc} ${-arc}
    c ${c} ${-d} ${b + c} ${-d} ${a + b + c} ${-d} Z`;
}
