import postcss from "postcss";
import { CORNER_CURVATURE, CORNER_RADIUS_SCALE } from "../src/theme/continuousCorners.ts";

const radiusProperty = /^border-(?:(?:top|bottom)-(?:left|right)-|(?:start|end)-(?:start|end)-)?radius$/;
const globalValue = /^(inherit|initial|unset|revert|revert-layer)$/;
const roundValue = /^(?:50%|(?:999|9999|10000)(?:px)?)$/;

/** Convert existing radius declarations once, after Tailwind has generated utilities. */
export default function continuousCorners() {
  return {
    postcssPlugin: "deepagent-continuous-corners",
    OnceExit(root) {
      root.walkAtRules("continuous-corners", rule => {
        const defaults = postcss.rule({ selector: ":root" });
        defaults.append({ prop: "--continuous-corner-scale", value: "1" });
        defaults.append({ prop: "--continuous-corner-curvature", value: "1" });
        const enabled = postcss.atRule({ name: "supports", params: "(corner-shape: superellipse(2))" });
        const tokens = postcss.rule({ selector: ":root" });
        tokens.append({ prop: "--continuous-corner-scale", value: String(CORNER_RADIUS_SCALE) });
        tokens.append({ prop: "--continuous-corner-curvature", value: String(CORNER_CURVATURE) });
        const surfaces = postcss.rule({ selector: ":where(*, *::before, *::after)" });
        surfaces.append({ prop: "corner-shape", value: "superellipse(var(--continuous-corner-curvature))" });
        enabled.append(tokens, surfaces);
        rule.replaceWith(defaults, enabled);
      });

      root.walkDecls(decl => {
        if (!radiusProperty.test(decl.prop)) return;
        if (globalValue.test(decl.value)) {
          if (decl.value === "inherit" && decl.prop === "border-radius") {
            decl.cloneAfter({ prop: "corner-shape", value: "inherit" });
          }
          return;
        }
        const axes = postcss.list.split(decl.value, ["/"]);
        const values = axes.flatMap(axis => postcss.list.space(axis));
        if (values.every(value => roundValue.test(value))) {
          decl.cloneAfter({ prop: "corner-shape", value: "round", important: decl.important });
          return;
        }
        // Percentages deliberately encode proportional/elliptical shapes. Preserve
        // those (e.g. the effort slider's jelly thumb), not just literal circles.
        if (decl.value.includes("%")) {
          decl.cloneAfter({ prop: "corner-shape", value: "round", important: decl.important });
          return;
        }
        decl.value = axes.map(axis => postcss.list.space(axis).map(value => {
          if (/^0(?:px|rem|em)?$/.test(value) || value.includes("--continuous-corner-scale")) return value;
          return `calc((${value}) * var(--continuous-corner-scale))`;
        }).join(" ")).join(" / ");
      });
    },
  };
}
continuousCorners.postcss = true;
