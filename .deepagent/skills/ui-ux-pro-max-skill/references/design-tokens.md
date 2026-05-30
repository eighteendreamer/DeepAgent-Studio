# UI UX Pro Max — Design Tokens & Guidelines

Reference material to consult while producing design-system recommendations.

## Color palette roles

Express palettes as CSS variables with explicit roles, not raw hex scattered in
markup:

```css
:root {
  --bg: ...;          /* page background */
  --surface: ...;     /* cards / panels */
  --text: ...;        /* primary text */
  --text-muted: ...;  /* secondary text */
  --primary: ...;     /* brand / primary action */
  --accent: ...;      /* sharp accent, used sparingly */
  --border: ...;
  --success / --warning / --danger: ...;
}
```

Dominant base + one sharp accent reads stronger than an evenly distributed
rainbow. Verify text/background contrast meets WCAG AA (4.5:1 body, 3:1 large).

## Type scale & pairing

- Pair a distinctive display font with a refined body font.
- Use a modular scale (e.g. 1.25 ratio): 12 / 14 / 16 / 20 / 25 / 31 / 39.
- Set line-height ~1.5 for body, ~1.1–1.25 for headings.

## Spacing & layout

- 4px base spacing unit; compose with multiples (4/8/12/16/24/32/48).
- Establish hierarchy with space and weight before color.

## Product-type → layout map

| Product type | Layout emphasis |
|--------------|-----------------|
| Marketing/landing | Hero + staggered reveals, strong type, one memorable moment |
| Dashboard | Information density, scannable cards, consistent chart grid |
| SaaS app | Persistent nav, predictable forms, empty/loading/error states |
| Docs | Generous reading measure (60–75ch), sticky TOC |

## Chart selection

| Question | Chart |
|----------|-------|
| Trend over time | Line / area |
| Compare categories | Bar / column |
| Part of whole | Stacked bar (prefer over pie for >3 parts) |
| Distribution | Histogram / box plot |
| Correlation | Scatter |

## UX guidelines (always)

- Every interactive state: default / hover / focus / active / disabled.
- Every async view: loading / empty / error / success.
- Keyboard navigable; visible focus rings; respect reduced-motion.
