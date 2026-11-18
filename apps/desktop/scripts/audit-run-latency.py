"""Read existing runtime timing without exporting prompts, responses or credentials."""
import argparse
import json
import sqlite3
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument("logs", type=Path)
parser.add_argument("--run-id")
args = parser.parse_args()
db = sqlite3.connect(args.logs.resolve().as_uri() + "?mode=ro", uri=True)
run_id = args.run_id or db.execute(
    "SELECT run_id FROM runtime_logs WHERE run_id IS NOT NULL ORDER BY id DESC LIMIT 1"
).fetchone()[0]
rows = db.execute(
    "SELECT ts_ms,event,data_json FROM runtime_logs WHERE run_id=? ORDER BY id", (run_id,)
).fetchall()
if not rows:
    raise SystemExit("No runtime log rows for this run")
allowed = {
    "model", "elapsed_ms", "duration_ms", "name", "step", "attempt", "reason",
    "event_type", "prompt_tokens", "completion_tokens", "message_count", "tool_count",
}
timeline = []
for timestamp, event, data in rows:
    payload = json.loads(data)
    metadata = {key: value for key, value in payload.items()
                if key in allowed and isinstance(value, (str, int, float, bool))}
    # Export a classified reason, never an arbitrary upstream error body.
    reason = metadata.pop("reason", "")
    if event == "model_attempt_reset":
        metadata["reason"] = ("model SSE idle timeout" if "model SSE idle timeout" in reason
                              else "other model retry (details retained in local logs)")
    timeline.append({"at_ms": timestamp - rows[0][0], "event": event, **metadata})
print(json.dumps({"run_id": run_id, "duration_ms": rows[-1][0] - rows[0][0],
                  "timeline": timeline}, ensure_ascii=False, indent=2))
