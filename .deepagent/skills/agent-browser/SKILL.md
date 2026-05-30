---
name: Agent Browser
description: This skill should be used when the user needs to "browse a website", "fill a form", "click a button", "take a screenshot", "scrape a page", "extract data from a web page", "test a web app", or perform "web automation". Browser automation via a CLI using accessibility-tree element refs (e.g. @e1, @e2) instead of full DOM, an alternative to Playwright MCP.
version: 0.1.0
---

# Agent Browser

Drive a headless browser deterministically from the command line for AI agents.
Interactions use accessibility-tree **refs** (`@e1`, `@e2`, …) captured from a
snapshot rather than raw selectors, which keeps context small and selections
reliable.

## When to use

Reach for this skill to navigate pages, fill and submit forms, click elements,
take screenshots, extract or assert page content, and test local or remote web
apps without a full Playwright MCP setup.

## Workflow

1. **Snapshot** the page to capture interactive element refs.
2. **Act** on refs: navigate, click `@ref`, type into `@ref`, select options.
3. **Capture** screenshots or extract text/attributes for assertions.
4. **Isolate** sessions so concurrent tasks don't collide.

## Notes

- Prefer refs over brittle CSS/XPath selectors; re-snapshot after navigation.
- Use isolated profiles/sessions for parallel automation.
- Keep each command single-purpose so failures are easy to localize.

## Bundled resources

- `references/commands.md` — the command catalog (navigate, snapshot, click,
  type, screenshot, extract) with ref-based usage examples.
