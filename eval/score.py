#!/usr/bin/env python3
"""Score local blinded agent answers and logged evidence volume."""
import argparse
import json
import math
from pathlib import Path

ROOT = Path(__file__).resolve().parent
EXPECTED = {
    "self_hotspot": {"frame": "parse", "weight": 40},
    "inclusive_region": {"frame": "dispatch", "weight": 90},
    "ambiguous_symbol": {"frames": ["compute<double>", "compute<int>"]},
    "dominant_caller": {"caller": "recursive", "weight": 15},
    "dominant_callee": {"callee": "parse", "weight": 40},
    "heavy_paths": {"paths": [
        {"frames": ["root", "dispatch", "parse"], "weight": 40},
        {"frames": ["root", "dispatch", "compute<int>"], "weight": 30},
        {"frames": ["root", "dispatch", "compute<double>"], "weight": 20}]},
    "recursion": {"frame": "recursive", "self_weight": 0, "inclusive_weight": 15},
    "diff": {"frame": "read", "baseline_weight": 10, "candidate_weight": 30,
             "delta_pp": 100 * (30 - 10) / 120},
    "large_profile": {"frame": "work4242", "weight": 1000},
    "cpp_identity": {"frames": [
        {"name": "ns::make<std::vector<double>>(double)", "weight": 40},
        {"name": "ns::make<std::vector<int>>(int)", "weight": 100}]},
    "tree_continuation": {"branch": "branch0", "leaf": "leaf0", "weight": 40},
    # Rejection evidence is reviewed separately: raw and CLI have no shared error-code API.
    "malformed_recovery": {"total_weight": 120},
}


def matches(expected, actual):
    if isinstance(expected, dict):
        return isinstance(actual, dict) and all(
            key in actual and matches(value, actual[key]) for key, value in expected.items())
    if isinstance(expected, list):
        return isinstance(actual, list) and len(expected) == len(actual) and all(
            matches(a, b) for a, b in zip(expected, actual))
    if isinstance(expected, float):
        return isinstance(actual, (int, float)) and math.isclose(expected, actual, abs_tol=1e-9)
    return expected == actual


def score(arm, trial):
    prefix = ROOT / "results" / f"{arm}-{trial}"
    answers = json.loads(Path(str(prefix) + "-answers.json").read_text())
    rejected = "empty frame" in json.dumps(answers).lower()
    answers = answers.get("answers", answers)
    logs = [json.loads(line) for line in Path(str(prefix) + "-queries.jsonl").read_text().splitlines()]
    failed = [key for key, value in EXPECTED.items() if not matches(value, answers.get(key))]
    if not rejected and "malformed_recovery" not in failed:
        failed.append("malformed_recovery")
    return {"arm": arm, "trial": trial, "correct": len(EXPECTED) - len(failed),
            "tasks": len(EXPECTED), "failed": failed, "evidence_calls": len(logs),
            "response_bytes": sum(item["response_bytes"] for item in logs),
            "byte_quarter_proxy": sum(item["token_estimate"] for item in logs),
            "authored_parser_bytes": sum(len(item.get("code", "").encode()) for item in logs)}


def self_check():
    assert matches({"weight": 15}, {"weight": 15, "notes": "extra"})
    assert not matches({"weight": 15}, {"weight": 30})
    assert not matches([1], [1, 2])
    assert matches(100 / 6, 16.666666666666664)
    assert not matches(1.0, float("nan"))
    rows = []
    for line in (ROOT / "corpus/baseline.folded").read_text().splitlines():
        stack, weight = line.rsplit(" ", 1)
        rows.append((stack.split(";"), int(weight)))
    assert sum(weight for _, weight in rows) == 120
    assert sum(weight for frames, weight in rows if "recursive" in frames) == 15
    assert sum(weight for frames, weight in rows if frames[-1] == "parse") == 40
    assert (ROOT / "corpus/malformed.folded").read_text().split(";")[1] == ""
    assert set(EXPECTED) == {task["id"] for task in json.loads((ROOT / "tasks.json").read_text())}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--self-check", action="store_true",
                        help="check scorer and fixtures without local trial results")
    args = parser.parse_args()
    self_check()
    if not args.self_check:
        results = [score(arm, trial) for arm in ("raw", "mcp") for trial in (1, 2)]
        print(json.dumps(results, indent=2))
        assert all(result["correct"] == result["tasks"] for result in results)
