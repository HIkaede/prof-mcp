#!/usr/bin/env python3
"""One MCP query against an isolated registered evaluation workspace."""
import argparse
import json
from pathlib import Path
import signal
import subprocess


def query(binary, workspace, tool, arguments):
    process = subprocess.Popen([str(binary), "serve"], cwd=workspace,
                               stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                               stderr=subprocess.DEVNULL, text=True)
    def request(identifier, method, params):
        process.stdin.write(json.dumps({"jsonrpc": "2.0", "id": identifier, "method": method, "params": params}) + "\n")
        process.stdin.flush()
        while True:
            line = process.stdout.readline()
            if not line:
                raise RuntimeError("MCP server closed stdout")
            response = json.loads(line)
            if response.get("id") == identifier:
                if "error" in response:
                    raise RuntimeError(json.dumps(response["error"]))
                return response["result"]
    def timeout(_signum, _frame):
        raise TimeoutError("MCP exchange exceeded 15 seconds")
    previous = signal.signal(signal.SIGALRM, timeout)
    signal.alarm(15)
    try:
        request(1, "initialize", {"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "prof-mcp-eval", "version": "1"}})
        process.stdin.write('{"jsonrpc":"2.0","method":"notifications/initialized"}\n')
        process.stdin.flush()
        if tool == "--list":
            result = request(2, "tools/list", {})
            return result
        result = request(2, "tools/call", {"name": tool, "arguments": arguments})
        value = result.get("structuredContent", result)
        payload = json.dumps(value, ensure_ascii=False, separators=(",", ":"))
        with (workspace / "queries.jsonl").open("a") as log:
            log.write(json.dumps({"tool": tool, "arguments": arguments, "response_bytes": len(payload.encode()), "token_estimate": (len(payload.encode()) + 3) // 4}) + "\n")
        return value
    finally:
        signal.alarm(0)
        signal.signal(signal.SIGALRM, previous)
        process.stdin.close()
        process.stdout.close()
        try:
            process.wait(timeout=2)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("workspace", type=Path)
    parser.add_argument("tool")
    parser.add_argument("arguments", nargs="?", default="{}")
    parser.add_argument("--binary", type=Path, default=Path(__file__).resolve().parents[1] / "target/debug/prof-mcp")
    args = parser.parse_args()
    print(json.dumps(query(args.binary, args.workspace.resolve(), args.tool, json.loads(args.arguments)), ensure_ascii=False))
