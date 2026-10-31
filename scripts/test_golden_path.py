"""Verify receipt authority in the HTTP crash acceptance scenario."""
import pathlib
import socket
import tempfile
import threading
import unittest
from unittest.mock import call, patch

import golden_path


class ReconciliationTests(unittest.TestCase):
    def test_waits_for_the_exact_run_and_answers_with_its_external_receipt(self):
        base = "http://receiver.invalid"
        run_id = "remote-run"
        key = "remote-run:4:0"
        result = {"topic": "Actix", "finding": "Recorded external result"}
        responses = [
            {"run": {"phase": "TOOL_CALL", "state": {"data": {}}}},
            {"run": {"phase": "WAITING", "state": {"data": {"reason": "timer"}}}},
            {"run": {"phase": "WAITING", "state": {"data": {
                "reason": "reconciliation", "key": key, "request_id": "confirmation"
            }}}},
            {"answered": True},
        ]
        with patch.object(golden_path, "api_request", side_effect=responses) as request:
            reconciled = golden_path.reconcile_uncertain_http(base, run_id, lambda: {key: result}, "subject-token")

        self.assertEqual(reconciled, key)
        self.assertEqual(request.call_args_list, [
            call(base, "/api/runs/remote-run", token="subject-token"),
            call(base, "/api/runs/remote-run", token="subject-token"),
            call(base, "/api/runs/remote-run", token="subject-token"),
            call(base, "/api/human-requests/confirmation/answer", {"result": result}, token="subject-token"),
        ])

    def test_another_invocations_receipt_cannot_answer_the_confirmation(self):
        response = {"run": {"phase": "WAITING", "state": {"data": {
            "reason": "reconciliation", "key": "remote-run:4:0", "request_id": "confirmation"
        }}}}
        with patch.object(golden_path, "api_request", return_value=response) as request:
            with self.assertRaisesRegex(AssertionError, "exact external invocation receipt"):
                golden_path.reconcile_uncertain_http(
                    "http://receiver.invalid", "remote-run", lambda: {"other-run:4:0": {"ok": True}}, "subject-token"
                )
        request.assert_called_once_with("http://receiver.invalid", "/api/runs/remote-run", token="subject-token")


class FixtureApprovalTests(unittest.TestCase):
    def test_task_run_lookup_and_completion_use_the_selected_run(self):
        run = {"id": "run-1", "phase": "RUNNING"}
        with patch.object(golden_path, "api_request", side_effect=[
            {"items": [run], "next_cursor": None}, {"run": run},
            {"run": {"id": "run-1", "phase": "COMPLETED"}},
        ]) as request:
            self.assertEqual(golden_path.latest_task_run(self.base, "task-1", "subject-token"), run)
            self.assertIsNone(golden_path.completed_run(self.base, "run-1", "subject-token"))
            self.assertEqual(golden_path.completed_run(self.base, "run-1", "subject-token")["phase"], "COMPLETED")
        self.assertEqual(request.call_args_list, [
            call(self.base, "/api/tasks/task-1/runs", token="subject-token"),
            call(self.base, "/api/runs/run-1", token="subject-token"),
            call(self.base, "/api/runs/run-1", token="subject-token"),
        ])

    def test_external_requests_follow_a_short_page(self):
        prompt = "Approve this exact external tool action once? Tool: research-http@1.0.0; call: {}"
        responses = [{"items": [], "next_cursor": "hidden"},
                     {"items": [{"id": "h1", "kind": "APPROVAL_REQUIRED", "prompt": prompt}], "next_cursor": None}, {}]
        with patch.object(golden_path, "api_request", side_effect=responses) as request:
            golden_path.approve_fixture_external_requests(self.base, "subject-token")
        self.assertEqual(request.call_args_list, [
            call(self.base, "/api/human-requests", token="subject-token"),
            call(self.base, "/api/human-requests?cursor=hidden", token="subject-token"),
            call(self.base, "/api/human-requests/h1/answer", {"approved": True}, token="subject-token"),
        ])

    base = "http://node.invalid"

    def card(self, identifier, tool, **overrides):
        return {"id": identifier, "kind": "tool_call", "run_id": "run", "revision": 1, "tool": tool, **overrides}

    def test_a_run_on_this_node_is_allowed_once_for_owned_fixture_tools_only(self):
        page = {"items": [self.card("own", "research-http"), self.card("foreign", "production-payments"),
                          self.card("outbound", "research-http", kind="outbound")], "next_cursor": None}
        sent = []

        def request(base, path, body=None, token=None):
            if body is None:
                return page
            sent.append((path, body["choice"], token))
            return {"state": "approved"}
        with patch.object(golden_path, "api_request", side_effect=request):
            golden_path.approve_fixture_tool_calls(self.base, "subject-token")
        self.assertEqual(sent, [("/api/capabilities/approvals/own/decide", "allow_once", "subject-token")])

    def test_a_run_executing_for_another_node_is_answered_through_its_human_request(self):
        prompt = "Approve this exact external tool action once? Tool: research-http@1.0.0; call: {}"
        requests = [{"id": "h1", "kind": "APPROVAL_REQUIRED", "prompt": prompt},
                    {"id": "h2", "kind": "APPROVAL_REQUIRED", "prompt": prompt.replace("research-http", "payments")},
                    {"id": "h3", "kind": "APPROVAL_REQUIRED", "prompt": prompt, "response": {"approved": False}}]
        responses = [{"items": requests, "next_cursor": None}, {}]
        with patch.object(golden_path, "api_request", side_effect=responses) as request:
            golden_path.approve_fixture_external_requests(self.base, "subject-token")
        self.assertEqual(request.call_args_list, [
            call(self.base, "/api/human-requests", token="subject-token"),
            call(self.base, "/api/human-requests/h1/answer", {"approved": True}, token="subject-token"),
        ])


class KeylessFixtureProxyTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.upstream_file = pathlib.Path(directory.name) / "0"
        held = socket.socket()
        held.bind(("127.0.0.1", 0))
        self.reserved_port = held.getsockname()[1]
        self.proxy = golden_path.KeylessFixtureProxy(held, self.upstream_file)
        self.addCleanup(self.proxy.close)

    def test_keeps_the_reserved_port_and_forwards_to_the_published_upstream(self):
        with socket.socket() as other, self.assertRaises(OSError):
            other.bind(("127.0.0.1", self.reserved_port))
        upstream = socket.create_server(("127.0.0.1", 0))
        self.addCleanup(upstream.close)

        def echo():
            connection, _ = upstream.accept()
            with connection:
                connection.sendall(connection.recv(1024).upper())

        threading.Thread(target=echo, daemon=True).start()
        self.upstream_file.write_text(str(upstream.getsockname()[1]))
        self.proxy.start()
        with socket.create_connection(("127.0.0.1", self.reserved_port), timeout=5) as client:
            client.sendall(b"keyless")
            self.assertEqual(client.recv(1024), b"KEYLESS")

    def test_closes_connections_until_the_spec_publishes_its_port(self):
        self.proxy.start()
        with socket.create_connection(("127.0.0.1", self.reserved_port), timeout=5) as client:
            self.assertEqual(client.recv(1024), b"")


if __name__ == "__main__":
    unittest.main()
