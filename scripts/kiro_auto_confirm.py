import os
import sys
import time
from datetime import datetime
from pathlib import Path

from pywinauto import Desktop


ROOT = Path(r"G:\Code_Warehouse\DeepAgent-Studio")
LOG_PATH = ROOT / "kiro-auto-confirm.log"
STOP_PATH = ROOT / "kiro-auto-confirm.stop"
PID_PATH = ROOT / "kiro-auto-confirm.pid"

WINDOW_TITLE_RE = ".*Kiro"
POLL_SECONDS = 1.0
DEDUP_SECONDS = 4.0

# We only auto-confirm one-off execution prompts. We intentionally avoid
# clicking persistent-trust actions unless the non-trust equivalent is absent.
BUTTON_PRIORITIES = [
    "Accept command",
    "Run",
    "Trust command and accept",
    "Trust",
]


def log(message: str) -> None:
    timestamp = datetime.now().strftime("%Y-%m-%d %H:%M:%S")
    line = f"[{timestamp}] {message}"
    print(line, flush=True)
    with LOG_PATH.open("a", encoding="utf-8") as handle:
        handle.write(line + "\n")


def should_stop() -> bool:
    return STOP_PATH.exists()


def get_kiro_window():
    try:
        return Desktop(backend="uia").window(title_re=WINDOW_TITLE_RE)
    except Exception:
        return None


def is_actionable(button, window_rect) -> bool:
    try:
        if not button.is_visible() or not button.is_enabled():
            return False
        rect = button.rectangle()
        if rect.width() <= 0 or rect.height() <= 0:
            return False
        if rect.top < window_rect.top or rect.bottom > window_rect.bottom:
            return False
        if rect.left < window_rect.left or rect.right > window_rect.right:
            return False
        return True
    except Exception:
        return False


def find_candidate(window):
    try:
        window_rect = window.rectangle()
        buttons = []
        for button in window.descendants(control_type="Button"):
            name = (button.element_info.name or "").strip()
            if name in BUTTON_PRIORITIES and is_actionable(button, window_rect):
                rect = button.rectangle()
                buttons.append((name, rect.top, rect.left, button))
        if not buttons:
            return None

        # Prefer the lower-right approval controls, then the inline accept button.
        buttons.sort(
            key=lambda item: (
                BUTTON_PRIORITIES.index(item[0]),
                -item[1],
                -item[2],
            )
        )
        return buttons[0][3]
    except Exception:
        return None


def click_button(button) -> str:
    name = (button.element_info.name or "").strip()
    try:
        button.click_input()
        return name
    except Exception:
        button.invoke()
        return name


def main() -> int:
    STOP_PATH.unlink(missing_ok=True)
    PID_PATH.write_text(str(os.getpid()), encoding="utf-8")
    log("Kiro auto-confirm watcher started.")
    last_click_key = None
    last_click_at = 0.0

    while True:
        if should_stop():
            log("Stop file detected. Exiting watcher.")
            STOP_PATH.unlink(missing_ok=True)
            return 0

        window = get_kiro_window()
        if window is None or not window.exists():
            time.sleep(POLL_SECONDS)
            continue

        candidate = find_candidate(window)
        if candidate is None:
            time.sleep(POLL_SECONDS)
            continue

        rect = candidate.rectangle()
        name = (candidate.element_info.name or "").strip()
        click_key = (name, rect.left, rect.top, rect.right, rect.bottom)
        now = time.time()
        if click_key == last_click_key and now - last_click_at < DEDUP_SECONDS:
            time.sleep(POLL_SECONDS)
            continue

        clicked = click_button(candidate)
        last_click_key = click_key
        last_click_at = now
        log(f"Clicked '{clicked}' at {rect}.")
        time.sleep(POLL_SECONDS)


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except KeyboardInterrupt:
        log("Interrupted. Exiting watcher.")
        raise
