"""A Live preview's `serve` operation: long-lived, unpublishable and stoppable."""
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


def serve_request(port=3000, seconds=7200):
    return {"operation_id": str(uuid.uuid4()), "area_id": str(uuid.uuid4()), "epoch": 1,
            "digest": "fixture", "usage_tenant": "acme", "kind": "serve", "code": "python3 -m http.server 3000",
            "seconds": seconds, "files": [], "port": port}


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
        runner.root = Path(root)
        runner.config = {"maximum_seconds": 600, "preview_maximum_seconds": 7200,
                         "preview_port": 3000, "working_bytes": 1 << 20, "limits": {},
                         "image": "image@sha256:" + "a" * 64}
        runner.config.update(config or {})
        runner.lock = threading.RLock()
        runner.kube_json = Mock(side_effect=AssertionError("serve takes no Area writer slot"))
        runner.update = Mock()
        runner.aggregate = ActivityAggregate()
        return runner

    def test_serve_is_accepted_on_the_declared_non_privileged_port(self):
        with tempfile.TemporaryDirectory() as root:
            runner = self.runner(root)
            request = serve_request()
            request["files"] = [file_entry()]
            record = runner.accept(request)
            self.assertEqual(record["status"], "awaiting_files")
            self.assertEqual(record["port"], 3000)
            self.assertEqual(record["serve"]["status"], "starting")

    def test_serve_does_not_fence_the_area_for_later_turns(self):
        with tempfile.TemporaryDirectory() as root:
            runner = self.runner(root)
            request = serve_request()
            request["files"] = [file_entry()]
            runner.accept(request)
            runner.kube_json.assert_not_called()

    def test_a_privileged_or_foreign_port_is_refused(self):
        with tempfile.TemporaryDirectory() as root:
            runner = self.runner(root)
            for port in (80, 3001, 0, -1, "3000"):
                request = serve_request(port=port)
                request["files"] = [file_entry()]
                with self.assertRaises(Rejected) as rejected:
                    runner.accept(request)
                self.assertEqual(rejected.exception.args[0], 400)

    def test_a_serve_lifetime_may_exceed_the_operation_maximum(self):
        with tempfile.TemporaryDirectory() as root:
            runner = self.runner(root)
            request = serve_request(seconds=7200)
            request["files"] = [file_entry()]
            runner.accept(request)
            too_long = serve_request(seconds=7201)
            too_long["files"] = [file_entry()]
            with self.assertRaises(Rejected):
                runner.accept(too_long)

    def test_a_session_token_is_bound_to_the_serve_operation(self):
        with tempfile.TemporaryDirectory() as root:
            runner = self.runner(root)
            runner.config["egress"] = {"endpoint": "http://10.0.0.2:8950"}
            request = serve_request()
            request["files"] = [file_entry()]
            request["egress"] = {"url": "http://10.0.0.2:8950",
                                 "token": "session:" + request["operation_id"] + "." + "a" * 43}
            runner.accept(request)
            page = serve_request()
            page["files"] = [file_entry()]
            page["egress"] = {"url": "http://10.0.0.2:8950",
                              "token": "page:" + page["operation_id"] + "." + "a" * 43}
            with self.assertRaises(Rejected):
                runner.accept(page)
            other = serve_request()
            other["files"] = [file_entry()]
            other["egress"] = {"url": "http://10.0.0.2:8950",
                               "token": "session:" + str(uuid.uuid4()) + "." + "a" * 43}
            with self.assertRaises(Rejected):
                runner.accept(other)


class ServeManifestTest(unittest.TestCase):
    def runner(self, root):
        runner = object.__new__(Runner)
        runner.root = Path(root)
        runner.config = {"image": "image@sha256:" + "a" * 64, "runtime_class": "runsc",
                         "cpu": 1, "memory_bytes": 1 << 28, "processes": 32,
                         "working_bytes": 1 << 20, "temporary_bytes": 1 << 20,
                         "output_bytes": 1 << 20}
        return runner

    def record(self, kind="serve", port=3000):
        return {"operation_id": str(uuid.uuid4()), "area_id": str(uuid.uuid4()),
                "epoch": 1, "wire_digest": "digest",
                "request": {"kind": kind, "port": port, "files": [], "code": "serve"}}

    def test_a_serve_pod_declares_its_preview_port_and_a_tcp_readiness_probe(self):
        with tempfile.TemporaryDirectory() as root:
            manifest = self.runner(root).manifest(self.record())
            worker = manifest["spec"]["containers"][0]
            self.assertEqual(worker["ports"], [{"name": "preview", "containerPort": 3000}])
            self.assertEqual(worker["readinessProbe"]["tcpSocket"]["port"], 3000)
            self.assertEqual(manifest["metadata"]["labels"]["aidash-sandbox"], "true")

    def test_a_shell_pod_declares_no_preview_port(self):
        with tempfile.TemporaryDirectory() as root:
            manifest = self.runner(root).manifest(self.record("shell"))
            worker = manifest["spec"]["containers"][0]
            self.assertNotIn("ports", worker)
            self.assertNotIn("readinessProbe", worker)


class ServeReconcileTest(unittest.TestCase):
    def runner(self):
        runner = object.__new__(Runner)
        runner.lock = threading.RLock()
        runner.active = set()
        runner.config = {"maximum_seconds": 600, "preview_maximum_seconds": 7200,
                         "working_bytes": 1 << 20, "processes": 32, "cpu": 1,
                         "memory_bytes": 1 << 28, "temporary_bytes": 1 << 20,
                         "output_bytes": 1 << 20, "image": "image@sha256:" + "a" * 64,
                         "runtime_class": "runsc"}
        runner.state = {"operation_id": str(uuid.uuid4()), "status": "accepted",
                        "created_at": time.time(), "cancel_requested": False,
                        "executed": False, "area_id": str(uuid.uuid4()),
                        "wire_digest": "digest", "epoch": 1,
                        "request": {"kind": "serve", "port": 3000, "seconds": 7200,
                                    "files": [], "code": "python3 -m http.server 3000"}}
        runner.get = Mock(side_effect=lambda _: dict(runner.state))
        runner.update = Mock(side_effect=lambda _, **changes: runner.state.update(changes) or dict(runner.state))
        runner.pod = Mock(side_effect=[None, pod(), pod(), pod(), pod()])
        runner.kube = Mock(return_value=Mock(stdout=b"listening on 3000", stderr=b""))
        runner.kube_json = Mock()
        def exec_collector(_operation, action, data=None, timeout=45):
            if action == ["prepared"]:
                return Mock(stdout=json.dumps({"digest": None}).encode())
            return Mock(stdout=json.dumps({"accepted": True, "started_at": time.time()}).encode())

        runner.exec_collector = Mock(side_effect=exec_collector)
        runner.guard = Mock(return_value={"termination_confirmed": True})
        runner.startup_log = Mock(return_value="listening on 3000")
        return runner

    def test_a_cancelled_serve_stops_its_pod_and_confirms_termination(self):
        runner = self.runner()
        runner.state["cancel_requested"] = True
        runner.pod = Mock(side_effect=[None, pod(), None])
        runner.reconcile_serve(runner.state["operation_id"])
        self.assertEqual(runner.state["status"], "cancelled")
        self.assertTrue(runner.state["termination_confirmed"])
        self.assertEqual(runner.state["serve"]["reason"], "cancelled")

    def test_a_ready_serve_reports_its_pod_host_and_keeps_running(self):
        runner = self.runner()

        def cancel_once(_operation, **changes):
            runner.state.update(changes)
            if isinstance(changes.get("serve"), dict) and changes["serve"].get("status") == "running":
                runner.state["cancel_requested"] = True
            return dict(runner.state)

        runner.update = Mock(side_effect=cancel_once)
        runner.reconcile_serve(runner.state["operation_id"])
        self.assertEqual(runner.state["serve"]["pod_host"], "10.0.0.7")
        self.assertEqual(runner.state["status"], "cancelled")

    def test_a_serve_that_never_becomes_ready_fails_without_leaking_its_pod(self):
        runner = self.runner()
        runner.pod = Mock(return_value=pod(ready=False))
        runner.state["executed"] = True
        runner.state["running_at"] = time.monotonic() - 121
        with patch("control.time.sleep"):
            runner.reconcile_serve(runner.state["operation_id"])
        self.assertEqual(runner.state["status"], "failed")
        self.assertEqual(runner.state["serve"]["reason"], "port_timeout")
        runner.kube.assert_any_call(
            ["delete", "pod", "operation-" + runner.state["operation_id"], "--wait=true",
             "--timeout=60s"], timeout=70, check=False)


if __name__ == "__main__":
    unittest.main()
