#!/usr/bin/env python3
"""Linux benchmark: JSON report, independent z/2/3 builds and fixed synthetic workloads."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
CASES = {"small": (100, 3), "medium": (20_000, 3), "large": (200_000, 3),
         "high_cardinality": (50_000, 8), "deep": (100, 512)}
TOOLS = [("summary", {}), ("find_symbols", {"query": "work", "limit": 20}),
         ("top", {"limit": 20}), ("tree", {"max_depth": 4, "max_nodes": 64}),
         ("callers", {"frame": {"frame_name": "work42"}}),
         ("callees", {"frame": {"frame_name": "focus"}}),
         ("paths", {"frame": {"frame_name": "work42"}, "limit": 10}),
         ("diff", {"baseline": "base", "candidate": "candidate", "sort": "absolute", "limit": 20})]

EXTRA_TOOLS = [("top_scoped_dense", "top", {"profile": "changed", "frame": {"frame_name": "scope_dense"}, "limit": 20}),
               ("top_scoped_sparse", "top", {"frame": {"frame_name": "work42"}, "limit": 20}),
               ("paths_heap", "paths", {"frame": {"frame_name": "root"}, "limit": 10, "max_total_frames": 500}),
               ("diff_changed", "diff", {"baseline": "base", "candidate": "changed", "sort": "absolute", "limit": 20})]


def fixture(case, count, depth):
    for i in range(count):
        if case == "high_cardinality":
            frames = [f"unique{i}_{j}" for j in range(depth - 2)]
        else:
            frames = [f"bucket{j}_{i % 100}" for j in range(depth - 2)]
        if i % 100 == 42:
            frames[0] = "focus"
        yield ";".join(["root", *frames, f"work{i}"]) + f" {1000 if i == 42 else 1}\n"


def candidate_fixture(case, count, depth):
    for i, line in enumerate(fixture(case, count, depth)):
        if i == 0:
            continue
        names, weight = line.rstrip().rsplit(" ", 1)
        frames = names.split(";")
        if i % 2 == 0:
            frames.insert(1, "scope_dense")
        if i in (1, 42):
            weight = str(int(weight) + 500)
        yield ";".join(frames) + " " + weight + "\n"
    yield "root;added_work 7\n"


def timed(command, cwd, env=None):
    with tempfile.TemporaryDirectory() as directory:
        rss = Path(directory) / "rss"
        started = time.perf_counter()
        result = subprocess.run(["/usr/bin/time", "-f", "%M", "-o", str(rss), *map(str, command)],
                                cwd=cwd, env=env, capture_output=True, text=True)
        if result.returncode:
            raise RuntimeError(f"{command} failed ({result.returncode}):\n{result.stderr}")
        return {"wall_seconds": time.perf_counter() - started,
                "peak_rss_kib": int(rss.read_text().strip())}, result.stdout


def mcp(binary, workspace, repeats, tools=None, prime_aliases=("candidate", "changed"), capture=False, continuation_check_limit=128):
    with tempfile.TemporaryDirectory() as directory:
        rss = Path(directory) / "rss"
        process = subprocess.Popen(["/usr/bin/time", "-f", "%M", "-o", str(rss), str(binary), "serve"],
                                   cwd=workspace, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                   stderr=subprocess.DEVNULL, text=True, start_new_session=True)
        identifier = 0
        def request(method, params):
            nonlocal identifier
            identifier += 1
            process.stdin.write(json.dumps({"jsonrpc": "2.0", "id": identifier,
                                           "method": method, "params": params}) + "\n")
            process.stdin.flush()
            while True:
                line = process.stdout.readline()
                if not line:
                    raise RuntimeError("MCP server closed stdout")
                response = json.loads(line)
                if response.get("id") == identifier:
                    if "error" in response or (response["result"].get("isError") and not capture):
                        raise RuntimeError(response)
                    return len(line.encode()), response["result"]
        def timeout(_signum, _frame):
            raise TimeoutError("MCP benchmark exceeded 120 seconds")
        previous = signal.signal(signal.SIGALRM, timeout)
        signal.alarm(120)
        try:
            request("initialize", {"protocolVersion": "2025-11-25", "capabilities": {},
                                   "clientInfo": {"name": "benchmark", "version": "1"}})
            process.stdin.write('{"jsonrpc":"2.0","method":"notifications/initialized"}\n')
            process.stdin.flush()
            started = time.perf_counter()
            request("tools/call", {"name": "profile_summary", "arguments": {}})
            cold = time.perf_counter() - started
            tools_list_bytes, listing = request("tools/list", {})
            schemas = {tool["name"]: len(json.dumps(tool["outputSchema"], ensure_ascii=False, separators=(",", ":")).encode())
                       for tool in listing["tools"]}
            # Prime every blob before warm measurements.
            for alias in prime_aliases:
                request("tools/call", {"name": "profile_summary", "arguments": {"profile": alias}})
            rows = []
            outputs = {}
            for name, tool, arguments in tools if tools is not None else [(name, name, arguments) for name, arguments in TOOLS] + EXTRA_TOOLS:
                elapsed = 0.0
                for _ in range(repeats):
                    started = time.perf_counter()
                    size, result = request("tools/call", {"name": "profile_" + tool, "arguments": arguments})
                    elapsed += time.perf_counter() - started
                    if not result.get("isError"):
                        assert result["structuredContent"]["schema_version"] == "2"
                if result.get("isError"):
                    outputs[name] = {"isError": True, "error": json.loads(result["content"][0]["text"])}
                    rows.append({"operation": name, "roundtrip_seconds": elapsed / repeats,
                                 "wire_response_bytes": size, "isError": True, "error": outputs[name]["error"]})
                    continue
                if capture:
                    outputs[name] = result["structuredContent"]
                    continuations = []
                    def collect(value):
                        if isinstance(value, dict):
                            if isinstance(value.get("continuation"), dict):
                                continuations.append(value["continuation"])
                            for key, child in value.items():
                                if key != "continuation":
                                    collect(child)
                        elif isinstance(value, list):
                            for child in value:
                                collect(child)
                    collect(outputs[name])
                    for index, continuation in enumerate(continuations[:continuation_check_limit]):
                        _, resumed = request("tools/call", {"name": "profile_" + tool, "arguments": {
                            "continuation": continuation, "max_depth": 4, "max_nodes": 32, "min_scope_percent": 0.0}})
                        outputs[f"{name}_resume_{index}"] = resumed["structuredContent"] if "structuredContent" in resumed else {"isError": True, "error": json.loads(resumed["content"][0]["text"])}
                rows.append({"operation": name, "roundtrip_seconds": elapsed / repeats,
                             "wire_response_bytes": size,
                             "structured_content_bytes": len(json.dumps(result["structuredContent"], ensure_ascii=False, separators=(",", ":")).encode()),
                             "text_content_bytes": sum(len(item["text"].encode()) for item in result.get("content", []) if item["type"] == "text"),
                             "output_schema_bytes": schemas["profile_" + tool]})
        finally:
            signal.alarm(0)
            signal.signal(signal.SIGALRM, previous)
            process.stdin.close()
            process.stdout.close()
            try:
                process.wait(timeout=2)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait()
        return {"cold_summary_seconds": cold, "tools_list_wire_bytes": tools_list_bytes, "warm": rows,
                "process_peak_rss_kib": int(rss.read_text().strip()), **({"outputs": outputs} if capture else {})}


def self_check():
    assert len(TOOLS) == 8 and len({name for name, _ in TOOLS}) == 8
    assert len({name for name, _, _ in EXTRA_TOOLS}) == 4
    assert dict(TOOLS)["diff"]["sort"] == "absolute"
    assert next(args for name, _, args in EXTRA_TOOLS if name == "diff_changed")["sort"] == "absolute"
    for case, (_, depth) in CASES.items():
        lines = list(fixture(case, 50, depth))
        assert lines == list(fixture(case, 50, depth))
        assert len({line.rsplit(" ", 1)[0] for line in lines}) == 50
        assert all(len(line.rsplit(" ", 1)[0].split(";")) == depth for line in lines)
        assert sum(int(line.rsplit(" ", 1)[1]) for line in lines) == 1049
        candidate = list(candidate_fixture(case, 50, depth))
        assert candidate == list(candidate_fixture(case, 50, depth))
        assert len(candidate) == len(lines)
        assert sum(int(line.rsplit(" ", 1)[1]) for line in candidate) == 2055
        candidate_frames = [line.rsplit(" ", 1)[0].split(";") for line in candidate]
        assert not any("work0" in frames for frames in candidate_frames)
        assert sum("scope_dense" in frames for frames in candidate_frames) == 24
        assert candidate[-1] == "root;added_work 7\n"
        weights = {line.rsplit(" ", 1)[0].rsplit(";", 1)[-1]: int(line.rsplit(" ", 1)[1]) for line in candidate}
        assert weights["work1"] == 501 and weights["work42"] == 1500
    metrics, output = timed(["python3", "-c", "print('ok')"], ROOT)
    assert output == "ok\n" and metrics["peak_rss_kib"] > 0
    print("benchmark self-check passed")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--self-check", action="store_true")
    parser.add_argument("--output", type=Path, default=Path("/tmp/prof-mcp-benchmark.json"))
    parser.add_argument("--target-root", type=Path, default=ROOT / "target/benchmark")
    parser.add_argument("--skip-build", action="store_true", help="use previously built variants")
    parser.add_argument("--levels", nargs="+", choices=["z", "2", "3"], default=["z", "2", "3"])
    parser.add_argument("--repeats", type=int, default=5)
    parser.add_argument("--cases", nargs="+", choices=CASES, default=list(CASES))
    args = parser.parse_args()
    if args.self_check:
        self_check()
        return
    if args.repeats < 1:
        parser.error("--repeats must be positive")
    report = {"repeats": args.repeats, "host_rustc": subprocess.check_output(["rustc", "-Vv"], text=True),
              "scope": {"core_compute": "public JSON query including shaping; encode measured separately",
                        "mcp": "warm pipe roundtrip including registry checks, serialization and Python decoding",
                        "rss": "process high-water RSS; core includes baseline and changed model builds, unlike earlier single-model runs",
                        "output_cost": "UTF-8 bytes: compact structuredContent/schema, text strings and full wire response; not token counts",
                        "callees": "focus branch containing every hundredth stack; root would exceed working budgets",
                        "builds": "binary and benchmark example, including dependencies",
                        "diff": "original diff uses same bytes; diff_changed adds/removes frames and changes weights",
                        "scoped_top": "dense uses changed half-stack scope; sparse uses baseline single-stack leaf",
                        "paths_heap": "root posting, limit10,500-frame output budget",
                        "provenance": "host_rustc describes current host, not compiler of reused binaries",
                        "registry": "three aliases; summary registry metadata differs from original two-alias workload",
                        "cold": "cold in-process model cache; filesystem cache is not flushed"}, "variants": {}}
    for level in args.levels:
        target = args.target_root.resolve() / level
        env = dict(os.environ, CARGO_TARGET_DIR=str(target), CARGO_PROFILE_RELEASE_OPT_LEVEL=level)
        variant = {"builds": {}, "cases": {}, "build_provenance": "reused" if args.skip_build else "fresh"}
        if not args.skip_build:
            if target.exists():
                parser.error(f"{target} already exists; use --skip-build or a fresh --target-root")
            command = ["cargo", "build", "--locked", "--release", "--bin", "prof-mcp", "--example", "benchmark"]
            for kind in ["clean", "incremental_no_change"]:
                variant["builds"][kind], _ = timed(command, ROOT, env)
        binary = target / "release/prof-mcp"
        example = target / "release/examples/benchmark"
        variant["binary_bytes"] = binary.stat().st_size
        variant["binary_sha256"] = hashlib.sha256(binary.read_bytes()).hexdigest()
        variant["helper_sha256"] = hashlib.sha256(example.read_bytes()).hexdigest()
        for case in args.cases:
            with tempfile.TemporaryDirectory(prefix="prof-mcp-bench-") as directory:
                workspace = Path(directory)
                source = workspace / "input.folded"
                source.write_text("".join(fixture(case, *CASES[case])))
                candidate = workspace / "changed.folded"
                candidate.write_text("".join(candidate_fixture(case, *CASES[case])))
                metrics, output = timed([example, source, args.repeats, candidate], workspace)
                operations = [json.loads(line) for line in output.splitlines()]
                expected = {"build", *(name for name, _ in TOOLS), *(name for name, _, _ in EXTRA_TOOLS)}
                if {row["operation"] for row in operations} != expected:
                    raise RuntimeError("Rebuild the benchmark helper: expected build plus all twelve operations")
                for alias in ["base", "candidate"]:
                    subprocess.run([str(binary), "register", str(source), "--name", alias], cwd=workspace,
                                   check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                subprocess.run([str(binary), "register", str(candidate), "--name", "changed"], cwd=workspace,
                               check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                subprocess.run([str(binary), "use", "candidate"], cwd=workspace,
                               check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                variant["cases"][case] = {"input_bytes": source.stat().st_size,
                    "core_process": metrics, "core_operations": operations,
                    "mcp": mcp(binary, workspace, args.repeats)}
        report["variants"][level] = variant
        args.output.write_text(json.dumps(report, indent=2) + "\n")
    print(args.output.resolve())


if __name__ == "__main__":
    main()
