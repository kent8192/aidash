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
            reconciled = golden_path.reconcile_uncertain_http(base, run_id, lambda: {key: result})

        self.assertEqual(reconciled, key)
        self.assertEqual(request.call_args_list, [
            call(base, "/api/runs/remote-run"),
            call(base, "/api/runs/remote-run"),
            call(base, "/api/runs/remote-run"),
            call(base, "/api/human-requests/confirmation/answer", {"result": result}),
        ])

    def test_another_invocations_receipt_cannot_answer_the_confirmation(self):
        response = {"run": {"phase": "WAITING", "state": {"data": {
            "reason": "reconciliation", "key": "remote-run:4:0", "request_id": "confirmation"
        }}}}
        with patch.object(golden_path, "api_request", return_value=response) as request:
            with self.assertRaisesRegex(AssertionError, "exact external invocation receipt"):
                golden_path.reconcile_uncertain_http(
                    "http://receiver.invalid", "remote-run", lambda: {"other-run:4:0": {"ok": True}}
                )
        request.assert_called_once_with("http://receiver.invalid", "/api/runs/remote-run")


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
