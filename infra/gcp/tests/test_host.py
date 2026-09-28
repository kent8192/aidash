"""Activity/admission races using private temporary host state."""

from contextlib import ExitStack
import json
from pathlib import Path
import sys
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "control"))
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "runtime"))
import host


class HostTests(unittest.TestCase):
    def setUp(self):
        self.context = ExitStack()
        self.addCleanup(self.context.close)
        self.directory = Path(self.context.enter_context(TemporaryDirectory()))
        self.context.enter_context(patch.object(host, "ROOT", self.directory))
        self.context.enter_context(patch.object(host, "RUN", self.directory / "run"))
        self.context.enter_context(patch.object(host.time, "time", return_value=10000))
        self.context.enter_context(
            patch.object(
                host, "request", return_value=b'{"inflight":0,"last_active":0}'
            )
        )
        self.snapshot = dict(
            protocol="aidash-infra-activity/1",
            busy=False,
            counts={"runs": 0, "database_work": 0},
        )

    def record_previous(self, observed_at, busy):
        (self.directory / "last-active").write_text("100")
        (self.directory / "activity.json").write_text(
            json.dumps(dict(observed_at=observed_at, busy=busy))
        )

    def test_work_completion_starts_a_full_new_idle_hour(self):
        self.record_previous(9990, True)
        with patch.object(host, "snapshot", return_value=self.snapshot):
            result = host.observe()
        self.assertFalse(result["busy"])
        self.assertEqual(result["last_active"], 10000)

    def test_observation_gap_is_not_silently_counted_as_idle(self):
        self.record_previous(9800, False)
        with patch.object(host, "snapshot", return_value=self.snapshot):
            self.assertEqual(host.observe()["last_active"], 10000)

    def test_idle_database_maintenance_does_not_keep_a_host_alive(self):
        self.record_previous(9990, False)
        self.snapshot["counts"]["database_work"] = 1
        self.snapshot["busy"] = True
        with patch.object(host, "snapshot", return_value=self.snapshot):
            result = host.observe()
        self.assertFalse(result["busy"])
        self.assertEqual(result["last_active"], 100)

    def test_unconfirmed_runner_writer_is_busy_even_for_terminal_operation(self):
        (self.directory / "release.json").write_text(
            json.dumps({"images": {"observer": "fixture"}})
        )
        journal = self.directory / "journal"
        journal.mkdir()
        record = journal / "operation.json"
        record.write_text(
            json.dumps({"status": "uncertain", "termination_confirmed": False})
        )
        with patch.object(
            host, "command", return_value=json.dumps(self.snapshot).encode()
        ):
            self.assertTrue(host.snapshot()["busy"])
            record.write_text(
                json.dumps({"status": "completed", "writer_frozen": True})
            )
            self.assertFalse(host.snapshot()["busy"])

    def test_pause_failure_restores_admission_and_never_claims_sealed(self):
        calls = []

        def command(*args, **kwargs):
            calls.append(args)
            if args[:2] == ("docker", "inspect"):
                return b'{"Running": true, "Paused": false}'
            if args[:2] == ("docker", "pause"):
                raise RuntimeError("pause failed")
            return b""

        with patch.object(host, "command", command):
            with self.assertRaises(RuntimeError):
                host.seal()
        self.assertIn(("docker", "unpause", "aidash-app"), calls)
        self.assertFalse((self.directory / "run/sealed").exists())
        self.assertFalse((self.directory / "run/draining").exists())

    def test_seal_checks_uncommitted_database_work_after_pausing_admission(self):
        self.snapshot["busy"] = True
        self.snapshot["counts"]["database_work"] = 1
        with (
            patch.object(
                host, "command", return_value=b'{"Running": true, "Paused": false}'
            ),
            patch.object(host, "snapshot", return_value=self.snapshot),
        ):
            self.assertFalse(host.seal()["sealed"])
        self.assertFalse((self.directory / "run/sealed").exists())


if __name__ == "__main__":
    unittest.main()
