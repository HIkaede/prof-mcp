#!/usr/bin/env python3
"""Fixed folded workloads and serial cross-version measurements; builds happen separately."""
import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path
import random
import statistics
import sys
import subprocess

from benchmark import ROOT, CASES, fixture, candidate_fixture, timed, mcp

EXTRA_CASES = {
    "deep_anchor": (3000, 512), "long_names": (20000, 4),
    "no_sharing": (10000, 8), "dense_scoped": (20000, 3), "duplicates": (2000, 5),
    "recursive": (2000, 8), "terminal_prefix": (512, 4),
    "equivalent_positions": (2000, 8), "interleaved": (2000, 6),
    "wide": (10000, 4), "tied": (2000, 4),
    "empty": (0, 0), "minimum": (1, 1), "depth_limit": (1, 4096),
}
ALL_CASES = {**CASES, **EXTRA_CASES}


def workload(case, count, depth):
    if case in CASES:
        yield from fixture(case, count, depth)
        return
    if case == "dense_scoped":
        yield from candidate_fixture("medium", count, depth)
        return
    if case == "duplicates":
        rows = []
        for i in range(count):
            frames = ["root", "scope_dense" if i % 4 else "other", "focus" if i % 100 == 42 else "middle", f"work{i}"]
            rows.extend([";".join(frames) + " 1\n"] * 8)
        random.Random(20261012).shuffle(rows)
        yield from rows
        return
    for i in range(count):
        if case == "deep_anchor":
            frames = ["focus" if j == 256 else f"shared{j:04}" for j in range(depth - 1)] + [f"work{i}"]
        elif case == "long_names":
            frames = ["root", "focus" if i % 100 == 42 else "common", "long_" + "x" * 160 + str(i), f"work{i}"]
        elif case == "no_sharing":
            frames = [f"unique{i}_{j}" for j in range(depth)]
        elif case == "recursive":
            patterns = [["root", "a", "b", "a"], ["root", "a", "a"], ["root", "x", "a", "y", "a", "a"], ["root", "b"]]
            frames = patterns[i % len(patterns)] + [f"work{i}"]
        elif case == "terminal_prefix":
            frames = ["root", "a", "b", f"work{i}"]
            if i < 3:
                frames = ["root", "a", "b"][:i + 1]
        elif case == "equivalent_positions":
            frames = ["root", *(["a", "b"] if i % 2 else ["pad", "b", "a", "a"]), f"work{i}"]
        elif case == "interleaved":
            frames = [f"root{i:04}", *((["focus", "middle", "focus"] if i % 4 == 1 else ["pad", "focus", "focus"]) if i % 2 else ["other"]), f"work{i}"]
        elif case == "wide":
            frames = ["root", "focus", f"child{i:06}", f"work{i}"]
            if i < 4:
                frames[2:2] = [f"deep{j}" for j in range(32)]
        elif case == "tied":
            frames = ["root", "scope_dense" if i % 4 else "other", "focus" if i % 100 == 42 else "middle", f"work{i}"]
        elif case == "minimum":
            frames = ["only"]
        elif case == "depth_limit":
            frames = [f"frame{j:04}" for j in range(depth)]
        else:
            raise ValueError(case)
        weight = 1 if case in ("tied", "no_sharing", "terminal_prefix", "depth_limit") else (1000 if i == 42 else i % 17 + 1)
        yield ";".join(frames) + f" {weight}\n"


def analyze(text):
    records = []
    stacks = Counter()
    for line in text.splitlines():
        names, weight = line.rsplit(" ", 1)
        frames = tuple(names.split(";"))
        records.append((frames, int(weight)))
        stacks[frames] += int(weight)
    membership = Counter()
    positions = {}
    branches = set()
    previous = ()
    logical = 1
    for frames in sorted(stacks):
        common = 0
        for a, b in zip(previous, frames):
            if a != b:
                break
            common += 1
        logical += len(frames) - common
        if common:
            branches.add(frames[:common])
        previous = frames
        seen = {}
        for pos, frame in enumerate(frames):
            if frame not in seen:
                seen[frame] = [pos, pos]
            seen[frame][1] = pos
        for frame, pair in seen.items():
            membership[frame] += 1
            positions.setdefault(frame, set()).add(tuple(pair))
    selectors = {}
    predicates = {"all": lambda n: n == len(stacks), "one": lambda n: n == 1 and len(stacks) > 1,
                  "sparse": lambda n: 1 < n < max(3, len(stacks) // 10),
                  "dense": lambda n: max(2, len(stacks) // 3) <= n < len(stacks)}
    for kind, predicate in predicates.items():
        options = sorted((name for name, n in membership.items() if predicate(n)),
                         key=lambda name: (name not in ("root", "focus", "a", "scope_dense", "work42"), name))
        if options:
            selectors[kind] = options[0]
    variable_all = sorted(name for name, pairs in positions.items() if len(pairs) > 1 and membership[name] == len(stacks))
    if variable_all:
        selectors["all_variable"] = variable_all[0]
    if "a" in membership and "b" in membership and membership["a"] == membership["b"] == len(stacks):
        selectors["equivalent_a"] = "a"
        selectors["equivalent_b"] = "b"
    return {"B": len(text.encode()), "R": len(records), "S": len(stacks), "F": len(membership),
            "L": sum(map(len, stacks)), "P": sum(membership.values()), "N": logical,
            "C": 1 + len(stacks) + len(branches.difference(stacks)),
            "total_weight": sum(stacks.values()), "max_depth": max(map(len, stacks), default=0),
            "scope_counts": {**{kind: sum(predicate(n) for n in membership.values()) for kind, predicate in predicates.items()}, "general": sum(1 < n < len(stacks) for n in membership.values())},
            "variable_position_frames": sum(len(pairs) > 1 for pairs in positions.values()),
            "variable_position_entries": sum(membership[name] for name, pairs in positions.items() if len(pairs) > 1),
            "recursive_stacks": sum(len(set(frames)) < len(frames) for frames in stacks)}, selectors


def tools_for(selectors):
    tools = [("summary", "summary", {}), ("find_symbols", "find_symbols", {"query": "", "limit": 20}),
             ("top", "top", {"limit": 20}),
             ("diff", "diff", {"baseline": "base", "candidate": "candidate", "sort": "absolute", "limit": 20})]
    for depth, nodes, percent in [(0, 1, 0), (1, 8, 0), (4, 64, 0), (16, 64, 0), (4, 8, 10)]:
        tools.append((f"tree_{depth}_{nodes}_{percent}", "tree", {"max_depth": depth, "max_nodes": nodes, "min_scope_percent": percent}))
    for kind, name in selectors.items():
        frame = {"frame_name": name}
        tools.append((f"top_{kind}", "top", {"frame": frame, "limit": 20}))
        tools.append((f"paths_{kind}", "paths", {"frame": frame, "limit": 10, "max_total_frames": 500}))
        for tool in ("callers", "callees"):
            tools.append((f"{tool}_{kind}", tool, {"frame": frame, "max_depth": 4, "max_nodes": 64, "min_scope_percent": 0}))
            tools.append((f"{tool}_{kind}_small", tool, {"frame": frame, "max_depth": 1, "max_nodes": 2, "min_scope_percent": 10}))
    return tools


def normalize(value):
    if isinstance(value, dict):
        return {key: normalize(child) for key, child in value.items() if not (key == "node_id" and child is not None) and not (key == "cursor" and value.get("tool") == "profile_tree")}
    if isinstance(value, list):
        return [normalize(child) for child in value]
    return value


def compare_outputs(baseline, candidate, allow_direction_improvement=False):
    improved = []
    for name, before in baseline.items():
        after = candidate.get(name)
        if before == after:
            continue
        if (allow_direction_improvement and name.startswith(("callers_", "callees_"))
                and before.get("isError") and before["error"].get("details", {}).get("resource") == "direction_nodes"
                and after and not after.get("isError")):
            def node_count(node):
                return 1 + sum(node_count(child) for child in node.get("children", []))
            budget = 2 if name.endswith("_small") else 32 if "_resume_" in name else 64
            assert after["schema_version"] == "2" and node_count(after["data"]["root"]) <= budget
            improved.append(name)
            continue
        raise AssertionError(f"semantic mismatch: {name}")
    unexpected = set(candidate).difference(baseline)
    assert all(any(name.startswith(operation + "_resume_") for operation in improved) for name in unexpected), unexpected
    return improved


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def run_variant(binary, helper, case_dir, source, selectors, rounds, repeats, continuation_check_limit=128, rss_queries=False):
    for alias in ("base", "candidate"):
        subprocess.run([str(binary), "register", str(source), "--name", alias], cwd=case_dir,
                       check=True, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    selector_path = case_dir / "selectors.json"
    selector_path.write_text(json.dumps(selectors, sort_keys=True))
    raw = []
    outputs = None
    for _ in range(rounds):
        build_metrics, build_output = timed([helper, source, "1", "--build-only"], case_dir)
        core_metrics, core_output = timed([helper, source, repeats, "--single-model", selector_path], case_dir)
        protocol = mcp(binary, case_dir, repeats, tools=tools_for(selectors), prime_aliases=("candidate",), capture=True, continuation_check_limit=continuation_check_limit)
        current = normalize(protocol.pop("outputs"))
        if outputs is not None:
            assert outputs == current, "non-deterministic MCP output"
        outputs = current
        rss_rows = []
        if rss_queries:
            operations = ["tree", *[f"{tool}_{kind}" for kind in selectors for tool in ("top", "callers", "callees")]]
            for operation in operations:
                for warm in (False, True):
                    _, rss_output = timed([helper, source, "1", "--query-rss-warm" if warm else "--query-rss", selector_path, operation], case_dir)
                    rss_rows.append(json.loads(rss_output.splitlines()[-1]))
        raw.append({"isolated_query_rss": rss_rows, "single_model_build_process": build_metrics, "build": json.loads(build_output),
                    "single_model_core_process": core_metrics, "core": [json.loads(line) for line in core_output.splitlines()], "mcp": protocol})
    values = [row["build"]["seconds"] for row in raw]
    return {"binary_path": str(binary), "helper_path": str(helper), "binary_sha256": sha(binary), "helper_sha256": sha(helper), "raw_rounds": raw,
            "build_seconds": {"median": statistics.median(values), "min": min(values), "max": max(values)}, "outputs": outputs}


def self_check():
    for case, (count, depth) in ALL_CASES.items():
        count = min(count, 100)
        text = "".join(workload(case, count, depth))
        assert text == "".join(workload(case, count, depth))
        stats, selectors = analyze(text)
        assert stats["S"] == count
        assert stats["C"] <= max(1, 2 * count)
        assert stats["P"] <= stats["L"] and stats["N"] >= stats["C"]
        if case == "duplicates":
            assert stats["R"] == count * 8 and stats["total_weight"] == count * 8
        if case in ("recursive", "equivalent_positions", "interleaved"):
            assert stats["recursive_stacks"] > 0 and stats["variable_position_frames"] > 0
        if case == "equivalent_positions":
            assert "all" in selectors and "all_variable" in selectors and "equivalent_a" in selectors and "equivalent_b" in selectors
        if case == "no_sharing":
            assert stats["N"] == count * depth + 1 and stats["F"] == count * depth
        if case == "terminal_prefix":
            assert stats["C"] == count + 1
        if case == "deep_anchor":
            assert stats["L"] == count * depth and stats["scope_counts"]["all"] == depth - 1
        if case == "dense_scoped":
            assert "dense" in selectors
        if case == "depth_limit":
            assert stats["max_depth"] == 4096
    assert normalize({"node_id": 1, "child": {"tool": "profile_tree", "cursor": [1], "weight": 3}}) == {"child": {"tool": "profile_tree", "weight": 3}}
    assert normalize({"tool": "profile_callers", "cursor": [1, 2]}) == {"tool": "profile_callers", "cursor": [1, 2]}
    assert normalize({"node_id": None, "name": "root"}) == {"node_id": None, "name": "root"}
    before = {"callees_all": {"isError": True, "error": {"details": {"resource": "direction_nodes"}}}}
    after = {"callees_all": {"schema_version": "2", "data": {"root": {"children": []}}}}
    assert compare_outputs(before, after, True) == ["callees_all"]
    try:
        compare_outputs(before, after)
    except AssertionError:
        pass
    else:
        raise AssertionError("budget improvements must be explicitly enabled")
    print("ref benchmark self-check passed")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--self-check", action="store_true")
    parser.add_argument("--allow-direction-budget-improvement", action="store_true", help="report old direction node-limit errors becoming bounded successful queries")
    parser.add_argument("--baseline-bin", type=Path)
    parser.add_argument("--baseline-helper", type=Path)
    parser.add_argument("--provenance", type=Path, help="source hashes, compiler and build command recorded before measurement")
    parser.add_argument("--baseline-report", type=Path, help="reuse matching fixed baseline raw rounds")
    parser.add_argument("--candidate-bin", type=Path)
    parser.add_argument("--candidate-helper", type=Path)
    parser.add_argument("--cases", nargs="+", choices=ALL_CASES, default=list(ALL_CASES))
    parser.add_argument("--input", action="append", default=[], metavar="NAME=PATH", help="additional existing folded input, copied into work directory and hashed")
    parser.add_argument("--continuation-check-limit", type=int, default=128, help="maximum immediate returned continuations to resume per query (128 checks all returned)")
    parser.add_argument("--query-rss", action="store_true", help="independent cold/warm query processes reset Linux VmHWM after profile load when permitted")
    parser.add_argument("--rounds", type=int, default=7)
    parser.add_argument("--repeats", type=int, default=20)
    parser.add_argument("--work-dir", type=Path, default=Path("/tmp/prof-ref-workloads"))
    parser.add_argument("--output", type=Path, default=Path("/tmp/prof-ref-results.json"))
    args = parser.parse_args()
    if args.self_check:
        self_check()
        return
    if not 1 <= args.continuation_check_limit <= 128:
        parser.error("continuation-check-limit must be between1and128")
    if args.rounds < 1 or args.repeats < 1:
        parser.error("rounds and repeats must be positive")
    if not args.baseline_bin or not args.baseline_helper:
        parser.error("baseline binary and helper are required; compile before measurement")
    if bool(args.candidate_bin) != bool(args.candidate_helper):
        parser.error("candidate binary and helper must be supplied together")
    external_inputs = {}
    for item in args.input:
        name, separator, path = item.partition("=")
        if not separator or not name or not all(c.isalnum() or c in "_-" for c in name) or name in ALL_CASES or name in external_inputs:
            parser.error("--input requires a unique safe NAME=PATH")
        external_inputs[name] = Path(path).resolve()
    args.work_dir.mkdir(parents=True, exist_ok=True)
    report = {"rounds": args.rounds, "repeats": args.repeats,
              "rustc": subprocess.check_output(["rustc", "-Vv"], text=True), "python": sys.version,
              "scope": {"build": "independent process, one model, includes file read/parse, no queries",
                        "allocation": "instrumented System allocator, requested bytes excluding allocator metadata; query peak includes typed output, JSON shaping and encoding",
                        "latency": "allocator instrumentation active for core; MCP binary has no allocator instrumentation",
                        "core_rss": "one model followed by queries; separate build process isolates build peak",
                        "mcp_rss": "whole server high-water, shared same-byte aliases, not isolated query RSS",
                        "retained": "query end excludes pre-existing allocations; per-selector top_workspace records typed/shaped persistent capacities separately",
                        "typed": "typed_compute excludes JSON shaping/encoding; cold means first invocation for operation, warm means remaining repeats",
                        "isolated_query_rss": "separate process per query, VmHWM reset after load (flag5); reports reset result, baseline RSS and post-query HWM, growth includes output serialization/status reading and excludes already resident malloc pages",
                        "order": "alternating baseline/candidate each independent round; cached baseline allowed only for exploratory stage comparisons",
                        "semantic_comparison": f"full MCP values except tree node_id/cursor representation; direction cursors exact; first {args.continuation_check_limit} immediate continuations resumed independently; no response truncation hidden"}, "cases": {}}
    if args.provenance:
        report["build_provenance"] = json.loads(args.provenance.read_text())
    previous = json.loads(args.baseline_report.read_text()) if args.baseline_report else None
    for case in [*args.cases, *external_inputs]:
        print(case, flush=True)
        case_dir = args.work_dir / case
        case_dir.mkdir(exist_ok=True)
        source = case_dir / "input.folded"
        if case in external_inputs:
            input_bytes = external_inputs[case].read_bytes()
            text = input_bytes.decode("utf-8")
            source.write_bytes(input_bytes)
        else:
            text = "".join(workload(case, *ALL_CASES[case]))
            source.write_text(text)
        structure, selectors = analyze(text)
        row = {"structure": structure, "selectors": selectors, "input_sha256": sha(source), "input_path": str(source.resolve()), "input_origin": str(external_inputs[case]) if case in external_inputs else "deterministic synthetic recipe"}
        if previous:
            assert previous["scope"]["semantic_comparison"] == report["scope"]["semantic_comparison"], "baseline uses different normalization/resume coverage"
            old = previous["cases"][case]
            assert old["input_sha256"] == row["input_sha256"] and old["selectors"] == selectors
            assert previous["rounds"] == args.rounds and previous["repeats"] == args.repeats
            assert old["baseline"]["binary_sha256"] == sha(args.baseline_bin)
            assert old["baseline"]["helper_sha256"] == sha(args.baseline_helper)
            row["baseline"] = old["baseline"]
        elif args.candidate_bin:
            variants = {"baseline": (args.baseline_bin, args.baseline_helper), "candidate": (args.candidate_bin, args.candidate_helper)}
            for round_index in range(args.rounds):
                order = list(variants) if round_index % 2 == 0 else list(reversed(variants))
                for label in order:
                    binary, helper = variants[label]
                    current = run_variant(binary.resolve(), helper.resolve(), case_dir, source.resolve(), selectors, 1, args.repeats, args.continuation_check_limit, args.query_rss)
                    if label not in row:
                        row[label] = current
                    else:
                        assert row[label]["outputs"] == current["outputs"], "non-deterministic round output"
                        row[label]["raw_rounds"].extend(current["raw_rounds"])
                row["direction_budget_improvements"] = compare_outputs(row["baseline"]["outputs"], row["candidate"]["outputs"], args.allow_direction_budget_improvement)
            for label in variants:
                values = [item["build"]["seconds"] for item in row[label]["raw_rounds"]]
                row[label]["build_seconds"] = {"median": statistics.median(values), "min": min(values), "max": max(values)}
        else:
            row["baseline"] = run_variant(args.baseline_bin.resolve(), args.baseline_helper.resolve(), case_dir, source.resolve(), selectors, args.rounds, args.repeats, args.continuation_check_limit, args.query_rss)
        if args.candidate_bin and previous:
            row["candidate"] = run_variant(args.candidate_bin.resolve(), args.candidate_helper.resolve(), case_dir, source.resolve(), selectors, args.rounds, args.repeats, args.continuation_check_limit, args.query_rss)
            row["direction_budget_improvements"] = compare_outputs(row["baseline"]["outputs"], row["candidate"]["outputs"], args.allow_direction_budget_improvement)
        for label in ("baseline", "candidate"):
            if label in row:
                for sample in row[label]["raw_rounds"]:
                    assert sample["build"]["total_weight"] == structure["total_weight"]
                    assert sample["build"]["max_depth"] == structure["max_depth"]
                    assert sample["build"]["cct_physical_nodes"] in (structure["N"], structure["C"])
        report["cases"][case] = row
        args.output.write_text(json.dumps(report, indent=2) + "\n")
    print(args.output.resolve())


if __name__ == "__main__":
    main()
