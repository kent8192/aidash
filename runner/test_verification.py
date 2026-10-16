"""A Completion Gate Check ("verify") runs like shell but never exports /work."""
import base64
import contextlib
import io
import json
import os
import tempfile
import threading
import time
import unittest
import uuid
from pathlib import Path
from unittest.mock import Mock, patch

os.environ.setdefault("AIDASH_OUTPUT_BYTES", "1024")
import collector
from control import Rejected, Runner, termination_time


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


class DeadlineTest(unittest.TestCase):
    """Whether a Check timed out is what its liveness deadline did, not its runtime."""

    def probe(self, root, control):
        handler = object.__new__(collector.Health)
        handler.server = Mock(startup_deadline=0)
        handler.send_response = Mock()
        handler.end_headers = Mock()
        with patch.multiple(collector, CONTROL=root / "control.json", EXPIRED=root / "deadline.expired"):
            collector.save(collector.CONTROL, control)
            handler.do_GET()
        return handler.send_response.call_args.args[0]

    def test_a_deadline_that_expires_while_running_is_recorded(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            self.assertEqual(self.probe(root, {"deadline": time.monotonic() + 60, "cancelled": False}), 200)
            self.assertFalse((root / "deadline.expired").exists())
            self.assertEqual(self.probe(root, {"deadline": time.monotonic() - 1, "cancelled": False}), 503)
            self.assertTrue((root / "deadline.expired").exists())

    def test_a_cancellation_is_not_a_timeout(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            self.assertEqual(self.probe(root, {"cancelled": True, "deadline": 0}), 503)
            self.assertFalse((root / "deadline.expired").exists())

    def watch(self, root, control, kind="verify", connections=0):
        with patch.multiple(collector, CONTROL=root / "control.json", REQUEST=root / "request.json",
                            EXPIRED=root / "deadline.expired", OUTPUT=root / "output.bin",
                            OUTPUT_STATE=root / "output.state", OUTPUT_TAIL=root / "output.tail"):
            collector.save(collector.CONTROL, control)
            collector.save(collector.REQUEST, {"kind": kind})
            collector.save(collector.OUTPUT_STATE, {"total": 0, "stored": 0, "connections": connections})
            return collector.expire_if_running()

    def test_a_deadline_passed_with_the_command_running_is_recorded_without_a_probe(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            self.assertFalse(self.watch(root, {"deadline": time.monotonic() + 60, "cancelled": False}, connections=1))
            self.assertFalse((root / "deadline.expired").exists())
            self.assertTrue(self.watch(root, {"deadline": time.monotonic() - 1, "cancelled": False}, connections=1))
            self.assertTrue((root / "deadline.expired").exists())

    def test_a_stream_closing_after_the_deadline_records_it(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            with patch.multiple(collector, CONTROL=root / "control.json", REQUEST=root / "request.json",
                                EXPIRED=root / "deadline.expired"):
                collector.save(collector.REQUEST, {"kind": "verify"})
                collector.save(collector.CONTROL, {"deadline": time.monotonic() + 60, "cancelled": False})
                collector.expire_if_late()
                self.assertFalse((root / "deadline.expired").exists())
                collector.save(collector.CONTROL, {"deadline": time.monotonic() - 1, "cancelled": True})
                collector.expire_if_late()
                self.assertFalse((root / "deadline.expired").exists())
                collector.save(collector.CONTROL, {"deadline": time.monotonic() - 1, "cancelled": False})
                collector.expire_if_late()
                self.assertTrue((root / "deadline.expired").exists())

    def test_a_command_that_exited_before_its_deadline_did_not_time_out(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            self.assertTrue(self.watch(root, {"deadline": time.monotonic() - 1, "cancelled": False}))
            self.assertFalse((root / "deadline.expired").exists())

    def test_the_watcher_records_no_timeout_for_cancellations_or_python_sessions(self):
        for control, kind in (({"deadline": 0, "cancelled": True}, "verify"),
                              ({"deadline": time.monotonic() - 1, "cancelled": False}, "python")):
            with tempfile.TemporaryDirectory() as root:
                root = Path(root)
                self.assertTrue(self.watch(root, control, kind=kind, connections=1))
                self.assertFalse((root / "deadline.expired").exists())

    def test_final_logs_report_the_deadline(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            with patch.multiple(collector, OUTPUT=root / "output.bin", OUTPUT_STATE=root / "output.state",
                                OUTPUT_TAIL=root / "output.tail", EXPIRED=root / "deadline.expired"):
                collector.append_output(b"slow\n")
                for expired in (False, True):
                    if expired:
                        (root / "deadline.expired").touch()
                    stdout = io.StringIO()
                    with patch.object(collector.sys, "argv", ["collector.py", "logs", "1000", "final", "tail"]), \
                            contextlib.redirect_stdout(stdout):
                        collector.logs(1000)
                    self.assertEqual(json.loads(stdout.getvalue())["deadline_expired"], expired)


if __name__ == "__main__":
    unittest.main()


class TerminationTimeTest(unittest.TestCase):
    """A Check is charged until its command stopped, not until the controller saw it."""

    STARTED = 1_800_000_000.4  # 2027-01-15T08:00:00.4Z

    def test_a_prompt_observation_keeps_its_sub_second_precision(self):
        stopped = termination_time({"finishedAt": "2027-01-15T08:00:02Z"}, self.STARTED, self.STARTED + 2.3)
        self.assertEqual(stopped, self.STARTED + 2.3)

    def test_a_late_observation_is_capped_by_the_containers_own_stop(self):
        # The controller restarted and saw the stop a minute late.
        stopped = termination_time({"finishedAt": "2027-01-15T08:00:02Z"}, self.STARTED, self.STARTED + 60)
        self.assertEqual(stopped, 1_800_000_003.0)

    def test_without_a_usable_stop_time_the_observation_stands(self):
        for terminated in ({}, {"finishedAt": None}, {"finishedAt": "not a time"},
                           # The node's clock is far behind the controller's.
                           {"finishedAt": "2027-01-15T07:59:00Z"}):
            with self.subTest(terminated=terminated):
                self.assertEqual(termination_time(terminated, self.STARTED, self.STARTED + 5), self.STARTED + 5)



class StartStampTest(unittest.TestCase):
    """A Check is charged from when its command was published, also after a restart."""

    def hydrate(self, root, incoming):
        stdout = io.StringIO()
        with patch.multiple(collector, CONTROL=root / "control.json", REQUEST=root / "request.json",
                            ROOTS={"working": root / "work", "references": root, "received": root}), \
                patch.object(collector.sys, "stdin", io.StringIO(json.dumps(incoming))), \
                contextlib.redirect_stdout(stdout):
            collector.hydrate()
        return json.loads(stdout.getvalue())

    def test_a_repeated_hydrate_returns_the_original_start(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            incoming = {"digest": "d", "kind": "verify", "code": "make test", "processes": 16,
                        "files": [], "working_bytes": 1 << 20, "seconds": 60}
            first = self.hydrate(root, incoming)
            self.assertTrue(first["accepted"])
            self.assertIsInstance(first["started_at"], float)
            self.assertEqual(self.hydrate(root, incoming)["started_at"], first["started_at"])
            # A controller that restarts after the command ended reads it back.
            stdout = io.StringIO()
            with patch.multiple(collector, CONTROL=root / "control.json", REQUEST=root / "request.json"), \
                    contextlib.redirect_stdout(stdout):
                collector.prepared()
            self.assertEqual(json.loads(stdout.getvalue()), {"digest": "d", "started_at": first["started_at"]})
