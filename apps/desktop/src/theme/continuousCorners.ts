import type { CSSProperties } from "react";

/** Shared Apple-style corner policy for CSS, inline styles, and animation targets. */
export const CONTINUOUS_CORNERS = Object.freeze({ radius: 24, smoothing: 0.6 });
export const CORNER_RADIUS_SCALE = 1 + CONTINUOUS_CORNERS.smoothing;

// Native superellipses approximate the approved Figma curve. Match its straight-edge
// extent and 45-degree inset rather than replacing it with the squircle keyword.
export const CORNER_CURVATURE = Math.log2(
  -Math.LN2 / Math.log(1 - (1 - Math.SQRT1_2) / CORNER_RADIUS_SCALE),
);

export function supportsContinuousCorners() {
  return typeof CSS !== "undefined" && CSS.supports("corner-shape", "superellipse(2)");
}

export function continuousRadius(radius: number, supported = supportsContinuousCorners()) {
  return supported ? radius * CORNER_RADIUS_SCALE : radius;
}

export function continuousCornerStyle(
  radius: number | string,
  supported = supportsContinuousCorners(),
): CSSProperties & { cornerShape?: string } {
  // These values intentionally describe circles/capsules, including canvas handles.
  if (typeof radius === "string") {
    return radius.includes("%")
      ? { borderRadius: radius, cornerShape: "round" }
      : { borderRadius: supported ? `calc((${radius}) * ${CORNER_RADIUS_SCALE})` : radius };
  }
  if (radius >= 999) return { borderRadius: radius, cornerShape: "round" };
  return { borderRadius: continuousRadius(radius, supported) };
}

export function computedCornerCurvature(style: CSSStyleDeclaration) {
  const shape = style.getPropertyValue("corner-shape");
  if (shape === "round" || !supportsContinuousCorners()) return 1;
  const match = shape.match(/superellipse\(([\d.]+)\)/);
  return match ? Number(match[1]) : CORNER_CURVATURE;
}
