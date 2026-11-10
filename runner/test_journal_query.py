"""Journal Query runs without a writable Working Area."""
import tempfile
import unittest
import hashlib
import copy
import os
import uuid
import io
from types import SimpleNamespace
from unittest.mock import Mock
from pathlib import Path
from unittest.mock import patch

import test_serve
os.environ.setdefault("AIDASH_OUTPUT_BYTES", "1024")
import collector
import journal_access
from control import Handler, Rejected, save


class JournalManifestTest(unittest.TestCase):
    def test_journal_query_cannot_write_the_working_or_reference_mounts(self):
        fixture = test_serve.ServeManifestTest()
        with tempfile.TemporaryDirectory() as root:
            manifest = fixture.runner(root).manifest(fixture.record("journal"))
        pod = manifest["spec"]
        self.assertEqual(manifest["metadata"]["labels"]["aidash-journal"], "true")
        worker = pod["containers"][0]
        mounts = {item["mountPath"]: item for item in worker["volumeMounts"]}
        self.assertTrue(mounts["/work"]["readOnly"])
        self.assertTrue(mounts["/references"]["readOnly"])
        self.assertTrue(mounts["/journal"]["readOnly"])
        self.assertFalse(mounts["/tmp"].get("readOnly", False))
        self.assertFalse(pod["hostNetwork"])
        self.assertEqual(pod["dnsConfig"]["nameservers"], ["127.0.0.1"])


class JournalRefreshTest(unittest.TestCase):
    def test_refresh_is_atomic_verified_and_safe_to_retry(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "journal.jsonl").write_bytes(b"old\n")
            token = "f3440a5b-f64a-4773-b33d-0d3c0ab0901f"
            new = b'{"seq":2}\n'
            (root / ".stage-old").write_bytes(b"withdrawn")
            with patch.object(collector, "JOURNAL", root):
                collector.stage_journal(token, 0, new[:4])
                self.assertFalse((root / ".stage-old").exists())
                collector.stage_journal(token, 0, new[:4])
                collector.stage_journal(token, 4, new[4:])
                with self.assertRaises(ValueError):
                    collector.commit_journal(token, len(new), "0" * 64)
                self.assertEqual((root / "journal.jsonl").read_bytes(), b"old\n")
                digest = hashlib.sha256(new).hexdigest()
                collector.commit_journal(token, len(new), digest)
                collector.commit_journal(token, len(new), digest)
                self.assertEqual((root / "journal.jsonl").read_bytes(), new)
                with patch.object(journal_access, "JOURNAL_PATH", root / "journal.jsonl"):
                    self.assertEqual(journal_access.journal(), [{"seq": 2}])
                    (root / "journal.jsonl").write_text('{"seq":3}\n')
                    self.assertEqual(journal_access.journal(), [{"seq": 3}])

    def test_conflicting_chunks_do_not_replace_installed_journal(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "journal.jsonl").write_bytes(b"old")
            token = "f3440a5b-f64a-4773-b33d-0d3c0ab0901f"
            with patch.object(collector, "JOURNAL", root):
                collector.stage_journal(token, 0, b"new")
                with self.assertRaises(ValueError):
                    collector.stage_journal(token, 0, b"bad")
                self.assertEqual((root / "journal.jsonl").read_bytes(), b"old")


class JournalSessionTest(unittest.TestCase):
    def test_session_absence_on_a_different_runner_does_not_prove_termination(self):
        fixture = test_serve.ServeAcceptTest()
        with tempfile.TemporaryDirectory() as directory:
            runner = fixture.runner(directory)
            runner.instance = "new-runner"
            with self.assertRaises(Rejected):
                runner.stop_session(str(uuid.uuid4()), "old-runner")

    def test_session_stop_http_checks_the_runner_instance(self):
        fixture = test_serve.ServeAcceptTest()
        with tempfile.TemporaryDirectory() as directory:
            runner = fixture.runner(directory)
            runner.instance, runner.token = "new-runner", "token"
            handler = object.__new__(Handler)
            handler.server = SimpleNamespace(runner=runner)
            handler.command = "POST"
            handler.path = "/v1/sessions/" + str(uuid.uuid4()) + "/stop"
            data = b'{"instance":"old-runner"}'
            handler.headers = {"Authorization": "Bearer token", "Content-Length": str(len(data))}
            handler.rfile = io.BytesIO(data)
            with self.assertRaises(Rejected):
                handler.handle_request()
            data = b'{"instance":"new-runner"}'
            handler.headers["Content-Length"] = str(len(data))
            handler.rfile = io.BytesIO(data)
            self.assertTrue(handler.handle_request()["termination_confirmed"])

    def test_reset_requires_positive_termination_confirmation(self):
        fixture = test_serve.ServeAcceptTest()
        with tempfile.TemporaryDirectory() as directory:
            runner = fixture.runner(directory, {"node_guard": ["guard"]})
            sid, operation = str(uuid.uuid4()), str(uuid.uuid4())
            path = Path(directory) / ("session-" + sid + ".json")
            save(path, {"session_id": sid, "operation_id": operation, "status": "frozen"})
            runner.get = Mock(return_value={"operation_id": operation})
            runner.guard = Mock(return_value={"termination_confirmed": False})
            runner.close_allocation = Mock()
            with self.assertRaises(RuntimeError):
                runner.stop_session(sid)
            self.assertEqual(__import__("json").loads(path.read_text())["status"], "frozen")
            runner.close_allocation.assert_not_called()

    def test_confirmed_stop_is_durable_and_safe_to_repeat(self):
        fixture = test_serve.ServeAcceptTest()
        with tempfile.TemporaryDirectory() as directory:
            runner = fixture.runner(directory, {"node_guard": ["guard"]})
            sid, operation = str(uuid.uuid4()), str(uuid.uuid4())
            path = Path(directory) / ("session-" + sid + ".json")
            save(path, {"session_id": sid, "operation_id": operation, "status": "frozen"})
            runner.get = Mock(return_value={"operation_id": operation})
            runner.guard = Mock(return_value={"termination_confirmed": True})
            runner.close_allocation = Mock()
            self.assertTrue(runner.stop_session(sid)["termination_confirmed"])
            runner.guard.side_effect = RuntimeError("Pod already removed")
            self.assertTrue(runner.stop_session(sid)["termination_confirmed"])

    def test_refresh_rejects_a_running_kernel_then_installs_verified_bytes_when_frozen(self):
        fixture = test_serve.ServeAcceptTest()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            runner = fixture.runner(directory)
            operation = str(uuid.uuid4())
            file_id = str(uuid.uuid4())
            (root / (operation + ".inputs")).mkdir()
            (root / (operation + ".inputs") / file_id).write_bytes(b"new\n")
            journal_dir = root / "journal"
            journal_dir.mkdir()
            (journal_dir / "journal.jsonl").write_bytes(b"old\n")
            record = {"operation_id": operation, "request": {"files": [{"file_id": file_id,
                      "size": 4, "digest": hashlib.sha256(b"new\n").hexdigest()}]}}
            def collector_exec(_, args, data=None):
                if args[0] == "journal-stage":
                    collector.stage_journal(args[1], int(args[2]), data)
                else:
                    collector.commit_journal(args[1], int(args[2]), args[3])
            runner.exec_collector = collector_exec
            runner.guard = Mock(return_value={"status": "running"})
            with patch.object(collector, "JOURNAL", journal_dir):
                with self.assertRaisesRegex(RuntimeError, "not frozen"):
                    runner.refresh_journal(record, "pod")
                self.assertEqual((journal_dir / "journal.jsonl").read_bytes(), b"old\n")
                runner.guard.return_value = {"status": "paused"}
                runner.refresh_journal(record, "pod")
                self.assertEqual((journal_dir / "journal.jsonl").read_bytes(), b"new\n")

    def test_reused_run_session_stages_a_new_export_before_executing(self):
        fixture = test_serve.ServeAcceptTest()
        with tempfile.TemporaryDirectory() as directory:
            runner = fixture.runner(directory, {"node_guard": ["guard"]})
            runner.instance = "runner-one"
            runner.kube_json = Mock(return_value={"items": []})
            runner.start = Mock()
            runner.begin_allocation_work = Mock()
            run = str(uuid.uuid4())
            sid = str(uuid.uuid4())
            save(Path(directory) / ("session-" + sid + ".json"), {
                "session_id": sid, "area_id": run, "status": "frozen", "kind": "journal",
                "manifest": runner.file_identity([]), "pod_uid": "pod", "pod_operation": str(uuid.uuid4()),
                "container_id": "container"})
            file = {"file_id": str(uuid.uuid4()), "path": "journal.jsonl", "scope": "journal",
                    "size": 3, "digest": hashlib.sha256(b"new").hexdigest()}
            request = {"operation_id": str(uuid.uuid4()), "area_id": run, "epoch": 2,
                       "digest": "fixture", "usage_tenant": "acme", "kind": "journal",
                       "code": "journal()", "seconds": 10, "files": [file], "session_id": sid,
                       "runner_instance": "runner-one"}
            wrong_instance = copy.deepcopy(request)
            wrong_instance['runner_instance'] = 'runner-replaced'
            with self.assertRaises(Rejected):
                runner.accept(wrong_instance)
            wrong_kind = copy.deepcopy(request)
            wrong_kind['kind'] = 'python'
            with self.assertRaises(Rejected):
                runner.accept(wrong_kind)
            egress = copy.deepcopy(request)
            egress['egress'] = {'url': 'http://example.test', 'token': 'x'}
            with self.assertRaises(Rejected):
                runner.accept(egress)
            wrong_scope = copy.deepcopy(request)
            wrong_scope['files'][0]['scope'] = 'working'
            with self.assertRaises(Rejected):
                runner.accept(wrong_scope)
            result = runner.accept(request)
            self.assertEqual(result["status"], "awaiting_files")
            runner.start.assert_not_called()


if __name__ == "__main__":
    unittest.main()
