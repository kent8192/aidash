"""Deployment Administrator drill sequencing and the persisted fault-time acceptance boundary."""
import argparse
from datetime import datetime, timedelta, timezone
import json
from pathlib import Path
import subprocess
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import patch

from scripts import home_ledger_drill as drill


class LostLedgerDrillTests(unittest.TestCase):
    def test_recovery_requires_a_replacement_disk_and_never_deletes_the_old_ledger(self):
        with TemporaryDirectory() as root:
            directory = Path(root) / "ledger"
            directory.mkdir()
            anchor = directory / "epoch.cbor"
            anchor.write_bytes(b"permanent epoch")
            evidence = Path(root) / "receipt.json"
            evidence.write_text(json.dumps({"format": 1, "before": {"epoch": "same"}}))
            args = argparse.Namespace(phase="recover", directory=directory, evidence=evidence,
                                      writers_fenced=True, fault_at=drill.stamp(), binary="aidash")
            with self.assertRaisesRegex(ValueError, "empty replacement"), patch.object(drill.subprocess, "run") as run:
                drill.run(args)
            run.assert_not_called()
            self.assertEqual(anchor.read_bytes(), b"permanent epoch")

    def test_elapsed_acceptance_starts_at_the_fault_and_a_resume_retry_keeps_that_clock(self):
        for hours, passed in [(1, True), (3, False)]:
            with self.subTest(hours=hours), TemporaryDirectory() as root:
                directory = Path(root) / "replacement"
                directory.mkdir()
                evidence = Path(root) / "receipt.json"
                before = {"home": "aidash://fixture", "epoch": "same", "serving": True, "protected": True,
                          "restoring": False, "head": {"sequence": 5, "recovering": False}}
                evidence.write_text(json.dumps({"format": 1, "before": before}))
                fault = datetime.now(timezone.utc) - timedelta(hours=hours)
                args = argparse.Namespace(phase="recover", directory=directory, evidence=evidence,
                                          writers_fenced=True, fault_at=fault.isoformat(), binary="aidash")
                report = {"home": before["home"], "epoch": "same", "head": {"sequence": 6, "recovering": True}}
                def result(value=None, code=0):
                    return subprocess.CompletedProcess([], code, json.dumps(value) if value else "resumed", "private diagnostic")
                with patch.object(drill.subprocess, "run", side_effect=[result(report["head"]), result(report), result(code=1)]) as calls:
                    with self.assertRaisesRegex(RuntimeError, "resume failed"):
                        drill.run(args)
                    self.assertEqual([call.args[0][2] for call in calls.call_args_list], ["reconstruct", "reconcile-all", "resume"])
                saved = json.loads(evidence.read_text())
                self.assertEqual(saved["fault_at"], fault.isoformat())
                self.assertNotIn("private diagnostic", evidence.read_text())
                args.fault_at = None
                after = {**before, "head": {"sequence": 7, "recovering": False}}
                with patch.object(drill.subprocess, "run", side_effect=[result(), result(after)]) as calls:
                    if passed:
                        drill.run(args)
                    else:
                        with self.assertRaisesRegex(RuntimeError, "two-hour"):
                            drill.run(args)
                    self.assertEqual([call.args[0][2] for call in calls.call_args_list], ["resume", "status"])
                receipt = json.loads(evidence.read_text())
                self.assertEqual(receipt["passed"], passed)
                self.assertGreaterEqual(receipt["elapsed_from_fault_seconds"], hours * 3600)
                self.assertEqual(receipt["fault_at"], saved["fault_at"])
