---
name: Webapp Testing
description: This skill should be used when the user wants to "test a web app", "write an end-to-end test", "verify frontend behavior", "capture a screenshot", "debug UI interactions", "automate the browser", or "check browser console logs" against a local development server. A Python Playwright-based toolkit for testing and automating local web applications.
version: 0.1.0
---

# Webapp Testing

Test and automate local web applications with Python Playwright. Use it to write
end-to-end tests, verify frontend behavior, capture screenshots, collect browser
console/network logs, and debug UI interactions against development servers.

## When to use

Use when validating a running local web app: asserting on rendered DOM,
automating clicks/forms/navigation, capturing screenshots or video, and
inspecting console/network output for root-cause analysis.

## Workflow

1. **Manage the server** — start the dev server(s) via a lifecycle helper so
   tests don't leave orphan processes; support multi-server (backend + frontend)
   setups.
2. **Write native Playwright** — prefer plain Python Playwright scripts over
   bespoke wrappers for clarity and control.
3. **Drive the UI** — navigate, fill forms, click, wait for selectors/network
   idle, then assert on DOM state.
4. **Capture evidence** — screenshots (and video where useful) for failures.
5. **Inspect logs** — dump console and network logs to diagnose flakiness.

## Best practices

- Wait on explicit conditions (selector visible, response received), never fixed
  sleeps.
- Make tests deterministic; seed data and control time where possible.
- Tear down servers in a `finally`/fixture so runs are repeatable.

## Bundled resources

- `references/playwright.md` — server-lifecycle helper pattern and a Playwright
  script skeleton with robust waiting and teardown.
