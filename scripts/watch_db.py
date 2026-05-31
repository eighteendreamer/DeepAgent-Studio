#!/usr/bin/env python3
"""
Watch the DeepAgent Studio SQLite DB in real time.

Polls the DB (read-only) at a fixed interval and prints only what *changed*
since the last tick: new sessions, new events (with payload), task transitions,
and row-count deltas. Run it in a terminal while you use the desktop app to
watch writes land live.

Usage:
  python scripts/watch_db.py                 # auto-locate, poll every 1s
  python scripts/watch_db.py --interval 0.5  # faster polling
  python scripts/watch_db.py --path X.db     # a specific DB file
  python scripts/watch_db.py --session SID   # only watch one session's events

The DB is opened read-only (mode=ro), so a running app is never disturbed. The
API key is not in this DB (it lives in the OS keychain), so nothing secret is
printed.
"""

import argparse
import datetime
import json
import os
import sqlite3
import sys
import time
from pathlib import Path

IDENTIFIER = "com.deepagent.studio"
DB_NAME = "deepagent.db"


def default_db_paths():
    candidates = []
    if sys.platform.startswith("win"):
        for var in ("APPDATA", "LOCALAPPDATA"):
            base = os.environ.get(var)
            if base:
                candidates.append(Path(base) / IDENTIFIER / DB_NAME)
    elif sys.platform == "darwin":
        candidates.append(
            Path.home() / "Library" / "Application Support" / IDENTIFIER / DB_NAME
        )
    else:
        candidates.append(Path.home() / ".local" / "share" / IDENTIFIER / DB_NAME)
    import tempfile

    candidates.append(Path(tempfile.gettempdir()) / DB_NAME)
    return candidates


def locate_db(explicit):
    if explicit:
        p = Path(explicit)
        if not p.exists():
            sys.exit(f"DB not found at: {p}")
        return p
    for p in default_db_paths():
        if p.exists():
            return p
    print("Could not auto-locate the DB. Looked in:")
    for p in default_db_paths():
        print(f"  - {p}")
    sys.exit("Pass --path <file> explicitly.")


# ANSI colors (fall back to nothing if not a TTY).
class C:
    enable = sys.stdout.isatty()

    @staticmethod
    def _w(code, s):
        return f"\033[{code}m{s}\033[0m" if C.enable else s

    @staticmethod
    def green(s):
        return C._w("32", s)

    @staticmethod
    def cyan(s):
        return C._w("36", s)

    @staticmethod
    def yellow(s):
        return C._w("33", s)

    @staticmethod
    def dim(s):
        return C._w("90", s)


def now():
    return datetime.datetime.now().strftime("%H:%M:%S")


def fmt_ts(ms):
    if ms is None:
        return "-"
    return datetime.datetime.fromtimestamp(ms / 1000).strftime("%H:%M:%S")


def connect_ro(db):
    # Read-only + nolock so we can read while the app holds a write lock (WAL).
    return sqlite3.connect(f"file:{db}?mode=ro", uri=True, timeout=1.0)


def snapshot(conn, session_filter):
    """Return (sessions: dict[id->row], max_seq_per_session, counts)."""
    cur = conn.cursor()
    sessions = {}
    where = ""
    params = ()
    if session_filter:
        where = "WHERE id = ?"
        params = (session_filter,)
    for row in cur.execute(
        f"""SELECT id, title, mode, project, created_at, updated_at, ended_at
            FROM sessions {where}""",
        params,
    ):
        sid, title, mode, project, created, updated, ended = row
        sessions[sid] = {
            "title": title,
            "mode": mode,
            "project": project,
            "updated": updated,
            "ended": ended,
        }
    counts = {}
    for t in ("sessions", "events", "tasks", "documents"):
        try:
            counts[t] = cur.execute(f"SELECT count(*) FROM {t}").fetchone()[0]
        except sqlite3.Error:
            counts[t] = -1
    return sessions, counts


def new_events_since(conn, last_seq, session_filter):
    """Yield events whose (session_id, sequence) is newer than last_seq map."""
    cur = conn.cursor()
    where = ""
    params = ()
    if session_filter:
        where = "WHERE session_id = ?"
        params = (session_filter,)
    rows = cur.execute(
        f"""SELECT session_id, sequence, kind, timestamp, payload
            FROM events {where} ORDER BY session_id, sequence""",
        params,
    ).fetchall()
    out = []
    for sid, seq, kind, ts, payload in rows:
        if seq > last_seq.get(sid, -1):
            out.append((sid, seq, kind, ts, payload))
            last_seq[sid] = seq
    return out


def short(sid):
    return sid[:12] + "…" if len(sid) > 13 else sid


def main():
    ap = argparse.ArgumentParser(description="Watch the DeepAgent Studio DB live")
    ap.add_argument("--path", help="explicit DB path")
    ap.add_argument("--interval", type=float, default=1.0, help="poll seconds")
    ap.add_argument("--session", help="only watch this session id")
    args = ap.parse_args()

    db = locate_db(args.path)
    print(C.cyan(f"Watching {db}"))
    print(C.dim(f"poll every {args.interval}s — Ctrl+C to stop\n"))

    prev_sessions = {}
    prev_counts = {}
    last_seq = {}
    first = True

    try:
        while True:
            try:
                conn = connect_ro(db)
            except sqlite3.Error as e:
                print(C.yellow(f"[{now()}] DB busy/locked: {e}"))
                time.sleep(args.interval)
                continue

            try:
                sessions, counts = snapshot(conn, args.session)
                events = new_events_since(conn, last_seq, args.session)
            except sqlite3.Error as e:
                print(C.yellow(f"[{now()}] read error: {e}"))
                conn.close()
                time.sleep(args.interval)
                continue

            if first:
                # Baseline: show current totals, prime last_seq, don't spam.
                print(
                    C.dim(
                        f"[{now()}] baseline — sessions={counts.get('sessions')} "
                        f"events={counts.get('events')} tasks={counts.get('tasks')} "
                        f"documents={counts.get('documents')}"
                    )
                )
                prev_sessions = sessions
                prev_counts = counts
                first = False
                conn.close()
                time.sleep(args.interval)
                continue

            # New sessions.
            for sid, s in sessions.items():
                if sid not in prev_sessions:
                    proj = s["project"] or "-"
                    print(
                        C.green(f"[{now()}] + SESSION {short(sid)}")
                        + f"  title={s['title']!r} project={proj}"
                    )

            # Ended sessions.
            for sid, s in sessions.items():
                was = prev_sessions.get(sid)
                if was and not was["ended"] and s["ended"]:
                    print(C.yellow(f"[{now()}] ✓ SESSION ENDED {short(sid)}"))

            # New events.
            for sid, seq, kind, ts, payload in events:
                line = C.cyan(f"[{now()}] » {short(sid)} #{seq} [{kind}]")
                detail = ""
                try:
                    obj = json.loads(payload)
                    # Compact, useful preview per kind.
                    if kind == "message_appended":
                        m = obj.get("message", {})
                        content = (m.get("content") or "").replace("\n", " ")
                        detail = f" {m.get('role')}: {content[:80]}"
                    elif kind == "tool_call_requested":
                        call = obj.get("call", {})
                        detail = f" {call.get('name')}({json.dumps(call.get('arguments', {}), ensure_ascii=False)[:60]})"
                    elif kind == "tool_call_completed":
                        detail = f" ok={obj.get('ok')} {obj.get('duration_ms')}ms"
                    elif kind == "task_created":
                        detail = f" goal={obj.get('goal', '')[:60]}"
                    elif kind == "task_state_changed":
                        detail = f" {obj.get('from')}→{obj.get('to')}"
                    elif kind == "session_started":
                        detail = f" title={obj.get('title')!r} mode={obj.get('mode')}"
                except Exception:
                    detail = ""
                print(line + detail)

            # Count deltas for documents (settings/mcp/projects writes).
            if counts.get("documents") != prev_counts.get("documents"):
                print(
                    C.dim(
                        f"[{now()}] documents {prev_counts.get('documents')} → {counts.get('documents')}"
                    )
                )

            prev_sessions = sessions
            prev_counts = counts
            conn.close()
            time.sleep(args.interval)
    except KeyboardInterrupt:
        print(C.dim("\nstopped."))


if __name__ == "__main__":
    main()
