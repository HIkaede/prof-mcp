#!/usr/bin/env python3
"""Run an evaluator-authored Python snippet and measure returned evidence."""
import contextlib
import io
import json
import os
from pathlib import Path
import sys

workspace = Path(sys.argv[1]).resolve()
code = sys.argv[2]
os.chdir(workspace)
output = io.StringIO()
with contextlib.redirect_stdout(output):
    exec(compile(code, "<raw-eval>", "exec"), {"__name__": "__main__"})
text = output.getvalue()
size = len(text.encode())
with (workspace / "queries.jsonl").open("a") as log:
    log.write(json.dumps({"tool": "raw_python", "code": code, "response_bytes": size, "token_estimate": (size + 3) // 4}) + "\n")
print(text, end="")
