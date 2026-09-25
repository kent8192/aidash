#!/usr/bin/env python3
"""Controller lifecycle regressions independent of cluster availability."""
import importlib.util
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


class ControllerLifecycle(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory(prefix="aidash-controller-test-")
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.events = []
        self.operation = str(uuid.uuid4())
        self.config = dict(journal=str(self.root), token_env="AIDASH_CONTROLLER_TEST_TOKEN",
                           namespace="test-runner", image="fixture@sha256:" + "a" * 64,
                           kubectl="unused", kubeconfig="unused", runtime_class="gvisor", node_guard=True)
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


if __name__ == "__main__":
    unittest.main()
