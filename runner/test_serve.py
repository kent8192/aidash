"""Runner fixtures shared by `/v1` operation tests. Live previews moved to Execution Pods (#745)."""
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
from control import ActivityAggregate, Rejected, Runner


def file_entry():
    return {"file_id": str(uuid.uuid4()), "path": "index.html", "scope": "working",
            "size": 10, "digest": "a" * 64}


def pod(uid="uid", ready=True, ip="10.0.0.7"):
    return {"metadata": {"uid": uid, "name": "operation-x"},
            "spec": {"nodeName": "node-1", "containers": [{"image": "image@sha256:" + "a" * 64}]},
            "status": {"podIP": ip,
                       "conditions": [{"type": "Ready", "status": "True" if ready else "False"}],
                       "containerStatuses": [
                           {"name": "execution", "containerID": "containerd://abc",
                            "state": {"running": {}}},
                           {"name": "collector", "containerID": "containerd://def",
                            "state": {"running": {}}}]}}


class ServeAcceptTest(unittest.TestCase):
    def runner(self, root, config=None):
        runner = object.__new__(Runner)
        runner.proven = True
        runner.root = Path(root)
        runner.config = {"maximum_seconds": 600, "preview_maximum_seconds": 7200,
                         "preview_port": 3000, "working_bytes": 1 << 20, "limits": {},
                         "image": "image@sha256:" + "a" * 64}
        runner.config.update(config or {})
        runner.lock = threading.RLock()
        runner.kube_json = Mock(side_effect=AssertionError("unexpected Pod lookup"))
        runner.update = Mock()
        runner.aggregate = ActivityAggregate()
        return runner


    def test_agent_commands_cells_and_previews_are_refused_on_v1(self):
        # They run in Execution Pods through /v2 (#745).
        with tempfile.TemporaryDirectory() as root:
            runner = self.runner(root)
            for kind, extra in (("python", {"session_id": str(uuid.uuid4())}), ("serve", {"port": 3000})):
                request = {"operation_id": str(uuid.uuid4()), "area_id": str(uuid.uuid4()), "epoch": 1,
                           "digest": "fixture", "usage_tenant": "acme", "kind": kind, "code": "x",
                           "seconds": 60, "files": [], **extra}
                with self.assertRaises(Rejected) as refused:
                    runner.accept(request)
                self.assertEqual(refused.exception.code, 400, kind)


class ServeManifestTest(unittest.TestCase):
    def runner(self, root):
        runner = object.__new__(Runner)
        runner.root = Path(root)
        runner.config = {"image": "image@sha256:" + "a" * 64, "runtime_class": "runsc",
                         "cpu": 1, "memory_bytes": 1 << 28, "processes": 32,
                         "working_bytes": 1 << 20, "temporary_bytes": 1 << 20,
                         "output_bytes": 1 << 20}
        return runner

    def record(self, kind="shell", port=3000):
        return {"operation_id": str(uuid.uuid4()), "area_id": str(uuid.uuid4()),
                "epoch": 1, "wire_digest": "digest",
                "request": {"kind": kind, "port": port, "files": [], "code": "serve"}}

    def test_a_shell_pod_declares_no_preview_port(self):
        with tempfile.TemporaryDirectory() as root:
            manifest = self.runner(root).manifest(self.record("shell"))
            worker = manifest["spec"]["containers"][0]
            self.assertNotIn("ports", worker)
            self.assertNotIn("readinessProbe", worker)


if __name__ == '__main__':
    unittest.main()
