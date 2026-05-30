# Agent Browser — Command Catalog

Ref-based browser automation. Capture a snapshot first, then act on the returned
element refs (`@e1`, `@e2`, …). Re-snapshot after any navigation.

## Session

- `open [--profile NAME] [--isolated]` — start a browser session.
- `close` — end the session and free resources.

## Navigation

- `goto <url>` — navigate to a URL.
- `back` / `forward` / `reload` — history controls.

## Inspection

- `snapshot` — capture the accessibility tree; returns element refs.
- `extract @ref [--attr name]` — read text or an attribute from an element.
- `logs` — dump console + network logs for debugging.

## Interaction

- `click @ref` — click an element.
- `type @ref "text"` — type into an input.
- `select @ref "option"` — choose a dropdown option.
- `screenshot [--full] [path]` — capture a PNG (viewport or full page).

## Best practices

- Always `snapshot` before acting; refs are only valid for the current DOM.
- Use `--isolated` sessions for parallel/independent automations.
- Assert on extracted text rather than on screenshots where possible.
