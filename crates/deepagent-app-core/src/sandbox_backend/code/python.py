# Transport/entrypoint adapter only; OS isolation is enforced by the host LPAC.
import io
import json
import sys

request = json.load(sys.stdin)
send = sys.stdout.write
flush = sys.stdout.flush
dumps = json.dumps


class BoundedLog(io.TextIOBase):
    def __init__(self):
        self.chunks = []
        self.size = 0
        self.truncated = False

    def write(self, text):
        data = str(text).encode("utf-8", "replace")
        remaining = max(0, request["logLimit"] - self.size)
        self.truncated |= len(data) > remaining
        self.chunks.append(data[:remaining]) if remaining else None
        self.size += min(remaining, len(data))
        return len(text)

    def value(self):
        return b"".join(self.chunks).decode("utf-8", "ignore")


stdout, stderr = BoundedLog(), BoundedLog()
sys.stdout, sys.stderr = stdout, stderr
try:
    compiled = compile(request["script"], "<workflow-code>", "exec")
except (SyntaxError, ValueError, OverflowError):
    send(dumps({"ok": False, "kind": "syntax"}))
    flush()
    raise SystemExit(1)

try:
    scope = {"__name__": "__workflow_code__"}
    exec(compiled, scope)
    outputs = scope["main"](request["inputs"])
    if not isinstance(outputs, dict):
        raise ValueError()
    frame = dumps({"ok": True, "outputs": outputs, "stdout": stdout.value(),
                   "stderr": stderr.value(), "stdout_truncated": stdout.truncated,
                   "stderr_truncated": stderr.truncated}, ensure_ascii=False, allow_nan=False)
    if len(frame.encode("utf-8")) > request["responseLimit"]:
        raise ValueError()
    send(frame)
    flush()
except BaseException:
    send(dumps({"ok": False, "kind": "runtime"}))
    flush()
    raise SystemExit(1)
