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
            self.assertEqual(result["data"]["rows"][0]["name"], "parse")
            self.assertEqual(result["data"]["rows"][0]["self_weight"], 40)
            self.assertEqual(result["data"]["metric"], "self")
            self.assertIsInstance(result["data"]["frame"], int)
            paths = query(BINARY, workspace, "profile_paths", {"frame": {"frame_name": "parse"}, "limit": 1})
            self.assertEqual(paths["data"]["paths"][0]["frames"], ["root", "parse"])
            self.assertIsInstance(paths["data"]["frame"], int)
            self.assertTrue((workspace / "queries.jsonl").is_file())

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
