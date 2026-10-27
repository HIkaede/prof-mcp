"""Transport checks for the evaluation harness (standard library only)."""
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

from query import query

BINARY = Path(__file__).resolve().parents[1] / "target/debug/prof-mcp"


class TransportTests(unittest.TestCase):
    def test_live_query(self):
        with tempfile.TemporaryDirectory() as directory:
            workspace = Path(directory)
            source = workspace / "sample.folded"
            source.write_text("root;parse 40\nroot;read 10\n")
            subprocess.run([str(BINARY), "register", str(source), "--name", "base"],
                           cwd=workspace, check=True, stdout=subprocess.DEVNULL)
            result = query(BINARY, workspace, "profile_top", {"metric": "self", "frame": {"frame_name": "root"}, "limit": 1})
            rows = result["data"]["rows"]
            first = dict(zip(rows["columns"], rows["rows"][0]))
            self.assertEqual(first["name"], "parse")
            self.assertEqual(first["self_weight"], 40)
            self.assertEqual(result["data"]["metric"], "self")
            self.assertIsInstance(result["data"]["frame"], int)
            paths = query(BINARY, workspace, "profile_paths", {"frame": {"frame_name": "parse"}, "limit": 1})
            self.assertEqual(paths["data"]["paths"][0]["frames"], ["root", "parse"])
            self.assertIsInstance(paths["data"]["frame"], int)
            self.assertTrue((workspace / "queries.jsonl").is_file())

    def test_benchmark_without_output_schemas(self):
        from benchmark import mcp
        with tempfile.TemporaryDirectory() as directory:
            workspace = Path(directory)
            source = workspace / "sample.folded"
            source.write_text("root;parse 40\nroot;read 10\n")
            subprocess.run([str(BINARY), "register", str(source), "--name", "base"],
                           cwd=workspace, check=True, stdout=subprocess.DEVNULL)
            popen = subprocess.Popen
            def start_without_time(argv, **kwargs):
                Path(argv[4]).write_text("0")
                return popen(argv[5:], **kwargs)
            with patch("benchmark.subprocess.Popen", side_effect=start_without_time):
                result = mcp(BINARY, workspace, 1, tools=[("summary", "summary", {})],
                             prime_aliases=("base",))
            self.assertEqual(result["warm"][0]["output_schema_bytes"], 0)
            self.assertGreater(result["warm"][0]["structured_content_bytes"], 0)

    def test_error_results(self):
        with tempfile.TemporaryDirectory() as directory:
            workspace = Path(directory)
            error = query(BINARY, workspace, "profile_summary", {})
            self.assertEqual(error["code"], "workspace_not_registered")
            self.assertTrue(error["retry_hint"])
            invalid = query(BINARY, workspace, "profile_top", {"metric": "unknown"})
            self.assertTrue(invalid["isError"])
            self.assertIn("failed to deserialize parameters", invalid["content"][0]["text"])

    def test_timeout_reaps_child(self):
        with tempfile.TemporaryDirectory() as directory:
            workspace = Path(directory)
            binary = workspace / "sleep.py"
            binary.write_text(f"#!{sys.executable}\nimport os,time\n"
                              "open('pid', 'w').write(str(os.getpid()))\ntime.sleep(60)\n")
            binary.chmod(0o700)
            alarm = signal.alarm
            with patch("query.signal.alarm", side_effect=lambda seconds: alarm(min(seconds, 1))):
                with self.assertRaises(TimeoutError):
                    query(binary, workspace, "profile_summary", {})
            pid = int((workspace / "pid").read_text())
            with self.assertRaises(ProcessLookupError):
                os.kill(pid, 0)


if __name__ == "__main__":
    unittest.main()
