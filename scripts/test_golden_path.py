"""Verify receipt authority in the HTTP crash acceptance scenario."""
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


if __name__ == "__main__":
    unittest.main()
