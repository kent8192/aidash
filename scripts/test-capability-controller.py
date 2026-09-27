#!/usr/bin/env python3
"""Controller lifecycle regressions independent of cluster availability."""
import importlib.util
import contextlib
import io
import sys
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import uuid

spec = importlib.util.spec_from_file_location("controller", Path(__file__).resolve().parents[1] / "runner/control.py")
controller = importlib.util.module_from_spec(spec)
spec.loader.exec_module(controller)


with patch.dict(os.environ, AIDASH_OUTPUT_BYTES="4096"):
    spec = importlib.util.spec_from_file_location("collector", Path(__file__).resolve().parents[1] / "runner/collector.py")
    collector = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(collector)
spec = importlib.util.spec_from_file_location("node_guard", Path(__file__).resolve().parents[1] / "runner/node_guard.py")
node_guard = importlib.util.module_from_spec(spec)
spec.loader.exec_module(node_guard)


class ControllerLifecycle(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory(prefix="aidash-controller-test-")
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.events = []
        self.operation = str(uuid.uuid4())
        self.config = dict(journal=str(self.root), token_env="AIDASH_CONTROLLER_TEST_TOKEN",
                           namespace="test-runner", image="fixture@sha256:" + "a" * 64,
                           kubectl="unused", kubeconfig="unused", runtime_class="gvisor", node_guard=True,
                           cpu=1, memory_bytes=1 << 28, working_bytes=1 << 20,
                           temporary_bytes=1 << 20, output_bytes=4096)
        self.env = patch.dict(os.environ, AIDASH_CONTROLLER_TEST_TOKEN="fixture-token-" + "0" * 32)
        self.env.start()
        self.addCleanup(self.env.stop)
        controller.save(self.root / f"{self.operation}.json", dict(operation_id=self.operation, status="accepted"))

    def start_controller(self, failure=None):
        events = self.events

        class FixtureRunner(controller.Runner):
            def kube_json(self, arguments, data=None):
                return {"handler": "runsc"}

            def ensure_network_policy(self):
                events.append("policy")

            def verify_isolation(self):
                events.append("isolation")
                if failure == "isolation":
                    raise RuntimeError("isolation rejected")
                self.verified = True

            def verify_freeze(self):
                events.append("freeze")
                if failure == "freeze":
                    raise RuntimeError("freeze rejected")
                self.python_verified = True

            def start(self, operation):
                events.append(("start", operation, self.verified, self.python_verified))

        runner = FixtureRunner.__new__(FixtureRunner)
        self.addCleanup(lambda: runner.owner.close())
        runner.__init__(self.config)
        return runner

    def test_recovery_waits_for_both_admission_checks(self):
        self.start_controller()
        self.assertEqual(self.events, ["policy", "isolation", "freeze", ("start", self.operation, True, True)])

    def test_failed_isolation_starts_no_journaled_work(self):
        with self.assertRaisesRegex(RuntimeError, "isolation rejected"):
            self.start_controller("isolation")
        self.assertEqual(self.events, ["policy", "isolation"])

    def test_failed_freeze_starts_no_journaled_work(self):
        with self.assertRaisesRegex(RuntimeError, "freeze rejected"):
            self.start_controller("freeze")
        self.assertEqual(self.events, ["policy", "isolation", "freeze"])

    def test_acknowledgement_releases_all_copied_payloads_and_is_idempotent(self):
        runner = self.start_controller()
        for status in ("completed", "cancelled", "failed"):
            with self.subTest(status=status):
                operation = str(uuid.uuid4())
                digest = "immutable-input-digest"
                controller.save(runner.path(operation), dict(operation_id=operation, digest=digest, status=status,
                    request={"code": "untrusted"}, stdout="large-output", files=[{"object_id": "original"}],
                    displays=[{"data": "copied-image-bytes"}]))
                for suffix in (".inputs", ".files"):
                    directory = self.root / (operation + suffix)
                    directory.mkdir()
                    (directory / "payload").write_bytes(b"copied original or output")
                result = runner.acknowledge(operation, digest)
                stored = json.loads(runner.path(operation).read_text())
                self.assertTrue(stored["acknowledged"])
                for key, empty in (("request", {}), ("stdout", ""), ("files", []), ("displays", [])):
                    self.assertEqual(stored[key], empty)
                self.assertEqual(stored["status"], status)
                self.assertEqual(runner.acknowledge(operation, digest), result)
                self.assertFalse((self.root / (operation + ".inputs")).exists())
                self.assertFalse((self.root / (operation + ".files")).exists())


    def test_readonly_mounts_reserve_the_aggregate_working_budget(self):
        runner = self.start_controller()
        for kind in ("shell", "python"):
            for mounted in (0, 4096, self.config["working_bytes"] - 1):
                with self.subTest(kind=kind, mounted=mounted):
                    request = dict(kind=kind, files=[dict(scope="references", size=mounted // 2),
                                                    dict(scope="received", size=mounted - mounted // 2),
                                                    dict(scope="working", size=0)])
                    record = dict(operation_id=self.operation, area_id=str(uuid.uuid4()), epoch=1,
                                  wire_digest="wire", request=request)
                    manifest = runner.manifest(record)
                    volume = next(v for v in manifest["spec"]["volumes"] if v["name"] == "work")
                    self.assertEqual(int(volume["emptyDir"]["sizeLimit"]), self.config["working_bytes"] - mounted)

        mounted = self.config["working_bytes"]
        files = [dict(file_id=str(uuid.uuid4()), path=f"input-{scope}.txt", scope=scope,
                      size=mounted // 2, digest="a" * 64)
                 for scope in ("references", "received")]
        record = dict(operation_id=self.operation, area_id=str(uuid.uuid4()), epoch=1,
                      wire_digest="wire", request=dict(kind="shell", files=files))
        with self.assertRaisesRegex(controller.Rejected, "no writable space"):
            runner.writable_bytes(record)
        request = dict(operation_id=self.operation, area_id=record["area_id"], epoch=1,
                       digest="caller", kind="shell", code="print('ok')", seconds=5,
                       files=files)
        runner.config["maximum_seconds"] = 60
        with self.assertRaisesRegex(controller.Rejected, "no writable space"):
            runner.accept(request)
        self.assertEqual(json.loads(runner.path(self.operation).read_text())["status"], "accepted")

    def test_exporters_reject_control_paths_and_preserve_printable_unicode(self):
        for index, (name, unsafe) in enumerate((("東京.txt", False), ("space name.txt", False), ("zero\u200bwidth.txt", False), ("bad\x1f.txt", True), ("bad\x7f.txt", True), ("bad\x85.txt", True), ("bad\x9f.txt", True))):
            with self.subTest(name=repr(name)):
                work = self.root / f"work-{index}"
                work.mkdir(exist_ok=True)
                path = work / name
                path.write_bytes(b"saved")
                self.addCleanup(lambda p=path: p.unlink(missing_ok=True))
                with patch.dict(collector.ROOTS, working=work), patch.object(sys, "argv", ["collector", "export", "4096"]):
                    if unsafe:
                        with self.assertRaisesRegex(ValueError, "invalid relative file path"):
                            collector.export()
                        with self.assertRaisesRegex(ValueError, "invalid file path"):
                            with node_guard.open_file(work, name):
                                pass
                    else:
                        output = io.StringIO()
                        with contextlib.redirect_stdout(output):
                            collector.export()
                        self.assertEqual(json.loads(output.getvalue())["files"][0]["path"], name)
                        with node_guard.open_file(work, name) as file:
                            self.assertEqual(file.read(), b"saved")
                path.unlink()


if __name__ == "__main__":
    unittest.main()
