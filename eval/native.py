#!/usr/bin/env python3
"""Probe a perf stackcollapse.py's event handler with deterministic samples."""
import contextlib
import io
import json
import os
from pathlib import Path
import runpy
import sys
import types


def collapse(script, events):
    # perf normally supplies these modules; this probe uses only process_event.
    for name in ("perf_trace_context", "Core", "EventClass"):
        sys.modules.setdefault(name, types.ModuleType(name))
    os.environ.setdefault("PERF_EXEC_PATH", str(script.parent))
    previous = sys.argv
    sys.argv = [str(script)]
    try:
        module = runpy.run_path(str(script))
    finally:
        sys.argv = previous
    for event in events:
        module["process_event"](event)
    output = io.StringIO()
    with contextlib.redirect_stdout(output):
        module["trace_end"]()
    return output.getvalue()


def sample(symbol, dso="/lib/A.so", pc=1, period=1, event="cycles", comm="app"):
    return {"comm": comm, "sample": {"pid": 1, "tid": 1, "period": period,
            "ip": pc}, "ev_name": event,
            "callchain": [{"sym": {"name": symbol}, "dso": dso, "ip": pc}]}


def probe(script):
    assert collapse(script, []) == ""
    assert collapse(script, [sample("f"), sample("f")]) == "app;f 2\n"
    results = {}
    for label, name in {"c": "malloc", "cpp": "ns::make<std::vector<int>>(int)",
                        "rust": "core::slice::sort::h1234", "unicode": "计算",
                        "operator": "operator<<", "long": "x" * 8192}.items():
        results[label] = collapse(script, [sample(name)]) == f"app;{name} 1\n"
    recursive = sample("f")
    recursive["callchain"] *= 3
    results["recursion"] = collapse(script, [recursive]) == "app;f;f;f 1\n"
    results["distinct_dso"] = len(collapse(script, [sample("foo"), sample("foo", "/lib/B.so")]).splitlines()) == 2
    results["distinct_unknown_pc"] = len(collapse(script, [sample(None), sample(None, pc=2)]).splitlines()) == 2
    results["distinct_semicolon"] = len(collapse(script, [sample("a;b"), sample("a:b")]).splitlines()) == 2
    results["distinct_process"] = len(collapse(script, [sample("f", comm="a b"), sample("f", comm="a_b")]).splitlines()) == 2
    results["period_weight"] = collapse(script, [sample("f", period=7)]) == "app;f 7\n"
    try:
        collapse(script, [sample("f"), sample("f", event="instructions")])
    except (ValueError, RuntimeError):
        results["mixed_event_rejected"] = True
    else:
        results["mixed_event_rejected"] = False
    return results


if __name__ == "__main__":
    if len(sys.argv) < 2:
        raise SystemExit("usage: native.py STACKCOLLAPSE.py [...]")
    print(json.dumps({str(Path(p)): probe(Path(p)) for p in sys.argv[1:]}, indent=2))
