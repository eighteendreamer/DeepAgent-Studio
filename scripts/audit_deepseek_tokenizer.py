"""Recount synthetic audit fixtures with the official downloaded tokenizer JSON.

No API calls, credentials, remote Python code, or source-file writes. Install
tokenizers into an isolated ignored directory and pass it with --python-packages.
The offline package is a version-pinned baseline, not proof of online token usage.
"""

import argparse
import hashlib
import json
import sys
from pathlib import Path


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tokenizer", required=True, type=Path)
    parser.add_argument("--python-packages", type=Path)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    if args.python_packages:
        sys.path.insert(0, str(args.python_packages.resolve()))
    import tokenizers

    fixture = Path(__file__).resolve().parents[1] / (
        "crates/deepagent-context/tests/fixtures/deepseek_tokenizer_samples.json"
    )
    spec = json.loads(fixture.read_text(encoding="utf-8"))
    actual_hash = hashlib.sha256(args.tokenizer.read_bytes()).hexdigest()
    if actual_hash != spec["tokenizer_sha256"]:
        raise SystemExit(f"tokenizer digest mismatch: {actual_hash}")
    if tokenizers.__version__ != spec["tokenizers_version"]:
        raise SystemExit(f"tokenizers version mismatch: {tokenizers.__version__}")
    tokenizer = tokenizers.Tokenizer.from_file(str(args.tokenizer))
    print(json.dumps({"tokenizer_sha256": actual_hash, "version": tokenizers.__version__}))
    failures = []
    for sample in spec["samples"]:
        value = sample["text"] * sample["repeat"]
        encoded = tokenizer.encode(value, add_special_tokens=False)
        if tokenizer.decode(encoded.ids) != value:
            raise SystemExit(f"roundtrip mismatch: {sample['name']}")
        count = len(encoded.ids)
        print(json.dumps({"sample": sample["name"], "characters": len(value),
                          "utf8_bytes": len(value.encode("utf-8")), "offline_tokens": count}))
        if args.check and count != sample["offline_tokens"]:
            failures.append(sample["name"])
    if failures:
        raise SystemExit(f"fixture token counts differ: {failures}")


if __name__ == "__main__":
    main()
