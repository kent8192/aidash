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
                recovering = {**before, "serving": False, "restoring": True, "head": report["head"]}
                def result(value=None, code=0):
                    return subprocess.CompletedProcess([], code, json.dumps(value) if value else "resumed", "private diagnostic")
                with patch.object(drill.subprocess, "run", side_effect=[result(report["head"]), result(report), result(recovering), result(code=1)]) as calls:
                    with self.assertRaisesRegex(RuntimeError, "resume failed"):
                        drill.run(args)
                    self.assertEqual([call.args[0][2] for call in calls.call_args_list], ["reconstruct", "reconcile-all", "status", "resume"])
                saved = json.loads(evidence.read_text())
                self.assertEqual(saved["fault_at"], fault.isoformat())
                self.assertNotIn("private diagnostic", evidence.read_text())
                args.fault_at = None
                after = {**before, "head": {"sequence": 7, "recovering": False}}
                with patch.object(drill.subprocess, "run", side_effect=[result(recovering), result(), result(after)]) as calls:
                    if passed:
                        drill.run(args)
                    else:
                        with self.assertRaisesRegex(RuntimeError, "two-hour"):
                            drill.run(args)
                    self.assertEqual([call.args[0][2] for call in calls.call_args_list], ["status", "resume", "status"])
                receipt = json.loads(evidence.read_text())
                self.assertEqual(receipt["passed"], passed)
                self.assertGreaterEqual(receipt["elapsed_from_fault_seconds"], hours * 3600)
                self.assertEqual(receipt["fault_at"], saved["fault_at"])

    def test_restart_recognizes_the_completed_resume_after_ack_status_or_receipt_loss(self):
        for lost in ("ack", "status", "receipt"):
            with self.subTest(lost=lost), TemporaryDirectory() as root:
                directory = Path(root) / "replacement"
                directory.mkdir()
                evidence = Path(root) / "receipt.json"
                before = {"home": "aidash://fixture", "epoch": "same", "serving": True, "protected": True,
                          "restoring": False, "head": {"sequence": 5, "recovering": False}}
                report = {"home": before["home"], "epoch": before["epoch"], "head": {"sequence": 6, "recovering": True}}
                evidence.write_text(json.dumps({"format": 1, "before": before}))
                fault = datetime.now(timezone.utc) - timedelta(hours=1)
                args = argparse.Namespace(phase="recover", directory=directory, evidence=evidence,
                                          writers_fenced=True, fault_at=fault.isoformat(), binary="aidash")
                resumed, interrupted = False, False
                actions = []

                def binary(argv, *, actions=actions, lost=lost, before=before, report=report, **kwargs):
                    nonlocal resumed, interrupted
                    action = argv[2]
                    actions.append(action)
                    if action == "resume":
                        if resumed:
                            return subprocess.CompletedProcess(argv, 1, "", "completed reconciliation report rejected")
                        resumed = True
                        if lost == "ack" and not interrupted:
                            interrupted = True
                            return subprocess.CompletedProcess(argv, 1, "", "private lost acknowledgement")
                        value = None
                    elif action == "status":
                        if resumed and lost == "status" and not interrupted:
                            interrupted = True
                            return subprocess.CompletedProcess(argv, 1, "", "private status unavailable")
                        value = ({**before, "head": {"sequence": 7, "recovering": False}} if resumed else
                                 {**before, "serving": False, "restoring": True, "head": report["head"]})
                    elif action == "reconstruct":
                        value = report["head"]
                    elif action == "reconcile-all":
                        value = report
                    else:
                        self.fail("unexpected maintenance action " + action)
                    return subprocess.CompletedProcess(argv, 0, json.dumps(value) if value else "resumed", "")

                save = drill.save

                def interrupted_save(path, value, *, lost=lost, save=save, **kwargs):
                    nonlocal interrupted
                    if lost == "receipt" and value.get("passed") and not interrupted:
                        interrupted = True
                        raise OSError("external receipt unavailable")
                    return save(path, value, **kwargs)

                with patch.object(drill.subprocess, "run", binary), patch.object(drill, "save", interrupted_save):
                    with self.assertRaises((RuntimeError, OSError)):
                        drill.run(args)
                    self.assertTrue(resumed)
                    self.assertNotIn("passed", json.loads(evidence.read_text()))
                    args.fault_at = None
                    drill.run(args)
                saved = json.loads(evidence.read_text())
                self.assertTrue(saved["passed"])
                self.assertEqual(saved["fault_at"], fault.isoformat())
                self.assertEqual(actions.count("resume"), 1, "completed reports cannot be submitted again")

    def test_restart_never_certifies_a_later_restore_or_an_unrelated_completed_head(self):
        for changed in (
            {"restoring": True}, {"protected": False}, {"serving": False}, {"home": "aidash://other"},
            {"epoch": "other"}, {"head": {"sequence": 8, "recovering": False}},
            {"head": {"sequence": 7, "recovering": True}},
        ):
            with self.subTest(changed=changed), TemporaryDirectory() as root:
                directory = Path(root) / "replacement"
                directory.mkdir()
                evidence = Path(root) / "receipt.json"
                before = {"home": "aidash://fixture", "epoch": "same", "serving": True, "protected": True,
                          "restoring": False, "head": {"sequence": 5, "recovering": False}}
                report = {"home": before["home"], "epoch": before["epoch"],
                          "head": {"sequence": 6, "recovering": True}}
                status = {**before, "head": {"sequence": 7, "recovering": False}, **changed}
                evidence.write_text(json.dumps({"format": 1, "before": before, "reconciliation": report,
                                                 "fault_at": drill.stamp(), "replacement": str(directory.resolve())}))
                args = argparse.Namespace(phase="recover", directory=directory, evidence=evidence,
                                          writers_fenced=True, fault_at=None, binary="aidash")

                def binary(argv, *, status=status, **kwargs):
                    if argv[2] == "status":
                        return subprocess.CompletedProcess(argv, 0, json.dumps(status), "")
                    self.assertEqual(argv[2], "resume")
                    return subprocess.CompletedProcess(argv, 1, "", "stale reconciliation report rejected")

                with patch.object(drill.subprocess, "run", binary):
                    with self.assertRaisesRegex(RuntimeError, "resume failed"):
                        drill.run(args)
                self.assertNotIn("passed", json.loads(evidence.read_text()))
