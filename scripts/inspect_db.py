#!/usr/bin/env python3
"""
Inspect the DeepAgent Studio SQLite database.

The desktop app stores its DB under the OS app-data dir:
  Windows:  %APPDATA%\\com.deepagent.studio\\deepagent.db
  macOS:    ~/Library/Application Support/com.deepagent.studio/deepagent.db
  Linux:    ~/.local/share/com.deepagent.studio/deepagent.db

Usage:
  python scripts/inspect_db.py                # auto-locate + summarize
  python scripts/inspect_db.py --path X.db    # a specific DB file
  python scripts/inspect_db.py --events SID   # full event stream for a session
  python scripts/inspect_db.py --sql "SELECT ..."   # run a read-only query

The API key is NOT in this DB (it lives in the OS keychain), so nothing secret
is printed here.
"""

import argparse
import json
import os
import sqlite3
import sys
from pathlib import Path

IDENTIFIER = "com.deepagent.studio"
DB_NAME = "deepagent.db"


def default_db_paths():
    """Candidate DB locations across platforms."""
    candidates = []
    if sys.platform.startswith("win"):
        appdata = os.environ.get("APPDATA")
        if appdata:
            candidates.append(Path(appdata) / IDENTIFIER / DB_NAME)
        local = os.environ.get("LOCALAPPDATA")
        if local:
            candidates.append(Path(local) / IDENTIFIER / DB_NAME)
    elif sys.platform == "darwin":
        candidates.append(
            Path.home() / "Library" / "Application Support" / IDENTIFIER / DB_NAME
        )
    else:
        candidates.append(Path.home() / ".local" / "share" / IDENTIFIER / DB_NAME)
    # Dev fallback: the temp dir (used when app_data_dir is unavailable).
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


def fmt_ts(ms):
    if ms is None:
        return "-"
    import datetime

    return datetime.datetime.fromtimestamp(ms / 1000).strftime("%Y-%m-%d %H:%M:%S")


def summarize(conn):
    cur = conn.cursor()

    # Tables overview.
    tables = [r[0] for r in cur.execute(
        "SELECT name FROM sqlite_master WHERE type='table' ORDER BY name"
    )]
    print(f"\n=== TABLES ({len(tables)}) ===")
    for t in tables:
        n = cur.execute(f"SELECT count(*) FROM {t}").fetchone()[0]
        print(f"  {t:<14} {n} rows")

    ver = cur.execute("PRAGMA user_version").fetchone()[0]
    print(f"\nschema version (user_version): {ver}")

    # Sessions.
    print("\n=== SESSIONS ===")
    rows = cur.execute(
        """SELECT id, title, mode, project, created_at, updated_at, ended_at
           FROM sessions ORDER BY updated_at DESC"""
    ).fetchall()
    if not rows:
        print("  (none)")
    for r in rows:
        sid, title, mode, project, created, updated, ended = r
        ev = cur.execute(
            "SELECT count(*) FROM events WHERE session_id=?", (sid,)
        ).fetchone()[0]
        proj = project or "-"
        print(f"  {sid}")
        print(f"      title={title!r} mode={mode} project={proj}")
        print(f"      events={ev} updated={fmt_ts(updated)} ended={'yes' if ended else 'no'}")

    # Distinct projects (the sidebar grouping).
    print("\n=== PROJECTS (distinct, from sessions) ===")
    prows = cur.execute(
        """SELECT project, count(*), MAX(updated_at)
           FROM sessions WHERE project IS NOT NULL AND project <> ''
           GROUP BY project ORDER BY MAX(updated_at) DESC"""
    ).fetchall()
    if not prows:
        print("  (none)")
    for project, cnt, last in prows:
        print(f"  {project}  ({cnt} sessions, last {fmt_ts(last)})")

    # Documents (settings / mcp / projects-registry / memory live here).
    print("\n=== DOCUMENTS (by collection) ===")
    drows = cur.execute(
        "SELECT collection, count(*) FROM documents GROUP BY collection ORDER BY collection"
    ).fetchall()
    if not drows:
        print("  (none)")
    for coll, cnt in drows:
        print(f"  {coll:<12} {cnt}")
    # Show the settings + project registry docs (no secrets in them).
    for coll, doc_id in [("settings", "app"), ("projects", "registry"), ("mcp", "servers")]:
        row = cur.execute(
            "SELECT body FROM documents WHERE collection=? AND id=?", (coll, doc_id)
        ).fetchone()
        if row:
            try:
                pretty = json.dumps(json.loads(row[0]), ensure_ascii=False, indent=2)
            except Exception:
                pretty = row[0]
            print(f"\n  --- {coll}/{doc_id} ---\n{pretty}")


def dump_events(conn, sid):
    cur = conn.cursor()
    rows = cur.execute(
        """SELECT sequence, kind, timestamp, payload
           FROM events WHERE session_id=? ORDER BY sequence ASC""",
        (sid,),
    ).fetchall()
    if not rows:
        sys.exit(f"No events for session {sid}")
    print(f"\n=== EVENTS for {sid} ({len(rows)}) ===")
    for seq, kind, ts, payload in rows:
        print(f"  #{seq} [{kind}] {fmt_ts(ts)}")
        try:
            print("     " + json.dumps(json.loads(payload), ensure_ascii=False))
        except Exception:
            print("     " + payload)


def run_sql(conn, sql):
    cur = conn.cursor()
    rows = cur.execute(sql).fetchall()
    cols = [d[0] for d in cur.description] if cur.description else []
    print("\t".join(cols))
    for r in rows:
        print("\t".join("" if v is None else str(v) for v in r))


def main():
    ap = argparse.ArgumentParser(description="Inspect the DeepAgent Studio DB")
    ap.add_argument("--path", help="explicit DB path")
    ap.add_argument("--events", metavar="SESSION_ID", help="dump a session's events")
    ap.add_argument("--sql", help="run a read-only SQL query")
    args = ap.parse_args()

    db = locate_db(args.path)
    print(f"DB: {db}  ({db.stat().st_size} bytes)")
    # Read-only connection (immutable so a running app can't be disturbed).
    conn = sqlite3.connect(f"file:{db}?mode=ro", uri=True)

    if args.sql:
        run_sql(conn, args.sql)
    elif args.events:
        dump_events(conn, args.events)
    else:
        summarize(conn)

    conn.close()


if __name__ == "__main__":
    main()
