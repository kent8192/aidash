"""A Completion Gate Check ("verify") runs like shell but never exports /work."""
import base64
import contextlib
import io
import json
import os
import tempfile
import threading
import unittest
import uuid
from pathlib import Path
from unittest.mock import Mock, patch

os.environ.setdefault("AIDASH_OUTPUT_BYTES", "1024")
import collector
from control import Rejected, Runner


def request(kind):
    return {"operation_id": str(uuid.uuid4()), "area_id": str(uuid.uuid4()), "epoch": 1,
            "digest": "fixture", "kind": kind, "code": "make test", "seconds": 60, "files": []}


class Admitted(Exception):
    """Raised once a request passed contract validation."""


class VerificationTest(unittest.TestCase):
    def runner(self, root):
        runner = object.__new__(Runner)
        runner.root = Path(root)
        runner.config = {"maximum_seconds": 600, "working_bytes": 1 << 20, "limits": {}}
        runner.kube = Mock()
        runner.writable_bytes = Mock(return_value=1 << 20)
        runner.update = Mock()
        return runner

    def test_only_known_command_kinds_are_accepted(self):
        with tempfile.TemporaryDirectory() as root:
            runner = self.runner(root)
            runner.lock = threading.RLock()
            runner.kube_json = Mock(side_effect=Admitted)
            for kind in ("shell", "verify"):
                with self.assertRaises(Admitted):
                    runner.accept(request(kind))
            with self.assertRaises(Rejected) as rejected:
                runner.accept(request("publish"))
            self.assertEqual(rejected.exception.args[0], 400)

    def stopped(self, kind):
        with tempfile.TemporaryDirectory() as root:
            runner = self.runner(root)
            calls = []

            def guard(record, action, **_):
                calls.append(action)
                if action == "status":
                    return {"termination_confirmed": True}
                if action == "export":
                    return {"files": []}
                return {"stdout": "", "truncated": False}

            runner.guard = guard
            record = {"operation_id": str(uuid.uuid4()), "request": request(kind)}
            runner.recover_stopped_shell(record)
            return calls, runner.update.call_args.kwargs

    def test_a_stopped_check_records_its_output_without_exporting_files(self):
        calls, recorded = self.stopped("verify")
        self.assertNotIn("export", calls)
        self.assertEqual(recorded["files"], [])
        self.assertEqual(recorded["status"], "failed")

    def test_a_stopped_shell_still_exports_its_files(self):
        calls, _ = self.stopped("shell")
        self.assertIn("export", calls)


class OutputTailTest(unittest.TestCase):
    def logs(self, *arguments):
        stdout = io.StringIO()
        with patch.object(collector.sys, "argv", ["collector.py", "logs", "1000", "final", *arguments]), \
                contextlib.redirect_stdout(stdout):
            collector.logs(1000)
        captured = json.loads(stdout.getvalue())
        return base64.b64decode(captured["stdout"]), captured["truncated"]

    def test_a_check_reads_the_end_of_output_the_stored_prefix_dropped(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            with patch.multiple(collector, OUTPUT=root / "output.bin", OUTPUT_STATE=root / "output.state",
                                OUTPUT_TAIL=root / "output.tail", OUTPUT_LIMIT=8, TAIL_LIMIT=12):
                collector.append_output(b"compiling crates\n")
                collector.append_output(b"test result: FAILED")
                self.assertEqual(self.logs("tail"), (b"sult: FAILED", True))
                self.assertEqual(self.logs(), (b"compilin", True))

    def test_a_short_check_output_is_returned_whole(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            with patch.multiple(collector, OUTPUT=root / "output.bin", OUTPUT_STATE=root / "output.state",
                                OUTPUT_TAIL=root / "output.tail", OUTPUT_LIMIT=64, TAIL_LIMIT=64):
                collector.append_output(b"ok\n")
                self.assertEqual(self.logs("tail"), (b"ok\n", False))


if __name__ == "__main__":
    unittest.main()
