"""Run with python3 tests/SummaryHookTests.py; model calls are stubbed."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
import unittest


HOOK = Path(__file__).resolve().parents[1] / "hooks/summarize.sh"


class SummaryHookTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="ccs-summary-")
        self.addCleanup(self.temp.cleanup)
        self.home = Path(self.temp.name)
        self.transcript = self.home / "rollout.jsonl"
        self.cache = self.home / ".cache/ccs"
        self.output = self.cache / "session-1.txt"
        self.calls = self.home / "calls.jsonl"
        self.env = dict(os.environ, HOME=str(self.home), PATH=f"{self.home}:{os.environ['PATH']}")
        self.env.pop("CCS_SUMMARIZING", None)
        stub = self.home / "codex"
        stub.write_text("""#!/usr/bin/env python3
import json, os, pathlib, sys
args = sys.argv[1:]
text = sys.stdin.read()
if not os.getenv('FAIL'):
    output = '' if os.getenv('EMPTY') else '\\n**Fixing login and verifying tests\\nExtra line\\n'
    if '--output-last-message' in args:
        pathlib.Path(args[args.index('--output-last-message') + 1]).write_text(output)
    else:
        print(output)
with open(pathlib.Path(os.environ['HOME']) / 'calls.jsonl', 'a') as f:
    f.write(json.dumps({'args': args, 'text': text, 'guard': os.getenv('CCS_SUMMARIZING')}) + '\\n')
sys.exit(1 if os.getenv('FAIL') else 0)
""")
        stub.chmod(0o755)
        (self.home / "claude").symlink_to(stub)

    def write_transcript(self, records):
        self.transcript.write_text("".join(json.dumps(r) + "\n" for r in records))

    def invoke(self, provider="codex", session_id="session-1"):
        subprocess.run([str(HOOK), *([provider] if provider else [])],
                       input=json.dumps({"session_id": session_id, "transcript_path": str(self.transcript)}),
                       text=True, env=self.env, check=True, capture_output=True, timeout=5)

    def wait_for(self, predicate):
        deadline = time.monotonic() + 5
        while not predicate():
            if time.monotonic() > deadline:
                self.fail("Background summary did not finish")
            time.sleep(0.01)

    def message(self, role, *texts):
        return {"type": "response_item", "payload": {"type": "message", "role": role,
                "content": [{"type": "output_text" if role == "assistant" else "input_text", "text": t}
                            for t in texts]}}

    def test_codex_conversation_and_throttle(self):
        self.write_transcript([
            self.message("developer", "Private coding instructions"),
            self.message("user", "# AGENTS.md instructions", "<environment_context>context</environment_context>"),
            self.message("user", "Fix login", "and verify tests"),
            self.message("assistant", "Checking the login handler", "Tests now pass"),
            {"type": "event_msg", "payload": {"type": "agent_message", "message": "Duplicate response"}},
            {"type": "response_item", "payload": {"type": "function_call_output", "output": "Tool output"}},
        ])
        self.invoke()
        self.wait_for(self.output.exists)
        self.assertEqual(self.output.read_text(), "Fixing login and verifying tests\n")
        call = json.loads(self.calls.read_text())
        self.assertEqual(call["text"], "<transcript>\nUSER: Fix login\nUSER: and verify tests\n"
                         "ASSISTANT: Checking the login handler\nASSISTANT: Tests now pass\n"
                         "</transcript>\n\nWrite the status line for the transcript above.")
        self.assertIn("gpt-6-luna", call["args"])
        for flag in ["--ephemeral", "--ignore-user-config", "--sandbox", "read-only", "hooks", "shell_tool"]:
            self.assertIn(flag, call["args"])
        self.assertEqual(call["guard"], "1")
        self.invoke()
        self.assertEqual(len(self.calls.read_text().splitlines()), 1)

    def test_failure_and_empty_output_preserve_previous_summary(self):
        self.write_transcript([self.message("user", "Fix login")])
        self.cache.mkdir(parents=True)
        self.output.write_text("Previous summary\n")
        for mode in ["FAIL", "EMPTY"]:
            self.env[mode] = "1"
            self.invoke()
            self.wait_for(lambda: self.calls.exists() and not list(self.cache.glob("*.tmp")))
            self.assertEqual(self.output.read_text(), "Previous summary\n")
            self.env.pop(mode)
            (self.cache / "session-1.stamp").unlink()
            self.calls.unlink()

    def test_claude_default_still_uses_haiku(self):
        self.write_transcript([
            {"type": "user", "message": {"content": "Fix login"}},
            {"type": "assistant", "message": {"content": [{"type": "text", "text": "Tests pass"}]}},
        ])
        self.invoke(provider=None)
        self.wait_for(self.output.exists)
        call = json.loads(self.calls.read_text())
        self.assertIn("haiku", call["args"])
        self.assertIn("USER: Fix login\nASSISTANT: Tests pass", call["text"])

    def test_invalid_missing_empty_and_recursive_inputs_do_not_call_model(self):
        self.invoke()  # missing transcript
        self.write_transcript([self.message("developer", "Only instructions")])
        self.invoke()
        self.invoke(session_id="../outside")
        self.write_transcript([self.message("user", "Fix login")])
        self.env["CCS_SUMMARIZING"] = "1"
        self.invoke(session_id="recursive")
        self.assertFalse(self.calls.exists())
        self.assertFalse((self.home / ".cache/outside.txt").exists())


if __name__ == "__main__":
    unittest.main()
