# Webapp Testing — Playwright Patterns

Native Python Playwright with disciplined server lifecycle and waiting.

## Server lifecycle helper

Wrap the dev server so it is started before the test and always stopped after,
even on failure. Support multiple servers (e.g. backend + frontend).

```python
import contextlib, subprocess, socket, time

def _wait_port(port, timeout=30):
    end = time.time() + timeout
    while time.time() < end:
        with contextlib.suppress(OSError):
            with socket.create_connection(("127.0.0.1", port), timeout=1):
                return
        time.sleep(0.25)
    raise TimeoutError(f"server on :{port} did not start")

@contextlib.contextmanager
def server(cmd, port):
    proc = subprocess.Popen(cmd, shell=True)
    try:
        _wait_port(port)
        yield
    finally:
        proc.terminate()
        with contextlib.suppress(Exception):
            proc.wait(timeout=10)
```

## Test skeleton

```python
from playwright.sync_api import sync_playwright, expect

with server("npm run dev", 5173):
    with sync_playwright() as p:
        browser = p.chromium.launch()
        page = browser.new_page()
        logs = []
        page.on("console", lambda m: logs.append(m.text))

        page.goto("http://127.0.0.1:5173")
        page.get_by_role("button", name="Add").click()
        # Wait on a condition, not a sleep:
        expect(page.get_by_test_id("count")).to_have_text("1")

        page.screenshot(path="after-add.png", full_page=True)
        browser.close()
```

## Rules

- Use `expect(...).to_*` auto-waiting assertions; avoid `time.sleep`.
- Prefer role/test-id locators over brittle CSS.
- Capture a screenshot + console logs on assertion failure for diagnosis.
- Always tear down the server (the `finally` in the context manager).
