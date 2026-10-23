"""Offline boundary tests; fixtures are not Stripe sandbox acceptance evidence."""

from contextlib import redirect_stdout
from copy import deepcopy
import importlib.util
import http.client
import io
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import urllib.error
import urllib.parse

SPEC = importlib.util.spec_from_file_location("stripe_spike", Path(__file__).with_name("stripe-sandbox-spike.py"))
spike = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(spike)
KEY = "sk_test_SyntheticCredentialNeverSentToStripe"


class Response(io.BytesIO):
    def __init__(self, payload):
        super().__init__(json.dumps(payload).encode())
        self.headers = {"Request-Id": "req_fixture"}


class SpikeBoundaryTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.previous = Path.cwd()
        os.chdir(self.temp.name)
        self.addCleanup(os.chdir, self.previous)
        Path("notes").mkdir()
        self.credentials = Path("credential.json")
        self.credentials.write_text(json.dumps({"STRIPE_SECRET_KEY": KEY}))
        self.credentials.chmod(0o600)
        self.output = Path("notes/result.json")

    def invoke(self, phase="prepare", transport=None, credentials=True):
        args = ["--output", str(self.output), "--phase", phase]
        if credentials:
            args += ["--credential-file", str(self.credentials)]
        stream = io.StringIO()
        with patch("urllib.request.OpenerDirector.open", side_effect=transport) as network, redirect_stdout(stream):
            code = spike.main(args)
        stdout = stream.getvalue()
        self.assertNotIn(KEY, stdout)
        self.assertNotIn(KEY, self.output.read_text())
        return code, json.loads(stdout), network

    def partial_state(self):
        state = spike.fresh_state()
        state["object_ids"] = {name: prefix + "fixture" for name, prefix in spike.ID_PREFIXES.items()
                               if name in ("product", "clock", "customer", "meter", "price")}
        self.output.write_text(json.dumps(state))
        return state

    def billing_state(self):
        state = self.partial_state()
        state["object_ids"].update(session="cs_test_fixture", subscription="sub_fixture")
        state["evidence"].update(checkout_complete=True, metered_checkout_accepted=True,
                                  meter_event_accepted=True, usage_aggregated=True,
                                  cycle_advanced=True, settlement_advanced=True,
                                  period_start=1800000000, period_end=1802678400,
                                  usage_timestamp=1800000000, aggregated_quantity=7)
        self.output.write_text(json.dumps(state))
        return state

    def billing_transport(self, state, mutation=None):
        ids, evidence = state["object_ids"], state["evidence"]
        payloads = {
            "checkout/sessions/cs_test_fixture": {
                "id": "cs_test_fixture", "livemode": False, "status": "complete",
                "mode": "subscription", "managed_payments": {"enabled": True},
                "metadata": {"aidash_spike": state["run_id"]},
                "customer": ids["customer"], "subscription": ids["subscription"]},
            "subscriptions/sub_fixture": {
                "livemode": False, "status": "active", "customer": ids["customer"],
                "metadata": {"aidash_spike": state["run_id"]}, "test_clock": ids["clock"],
                "items": {"has_more": False, "data": [{"price": {"id": ids["price"]},
                    "current_period_start": evidence["period_start"], "current_period_end": evidence["period_end"]}]}},
            "billing/meters/mtr_fixture/event_summaries": {
                "data": [{"livemode": False, "meter": ids["meter"], "aggregated_value": 7}], "has_more": False},
            "test_helpers/test_clocks/clock_fixture": {
                "livemode": False, "status": "ready", "name": "aidash-spike-" + state["run_id"],
                "frozen_time": evidence["period_end"] + 7200},
            "invoices": {"data": [{"id": "in_fixture", "livemode": False, "billing_reason": "subscription_cycle",
                "status": "paid", "currency": "usd", "amount_paid": 700, "amount_remaining": 0, "total": 700,
                "customer": ids["customer"], "test_clock": ids["clock"],
                "parent": {"subscription_details": {"subscription": ids["subscription"]}}}], "has_more": False},
            "invoices/in_fixture/lines": {"data": [{"id": "il_fixture", "livemode": False,
                "pricing": {"price_details": {"price": ids["price"]}}, "quantity": 7, "amount": 700,
                "period": {"start": evidence["period_start"], "end": evidence["period_end"]}}], "has_more": False},
            "invoice_payments": {"data": [{"livemode": False, "amount_paid": 700, "invoice": "in_fixture",
                "status": "paid", "currency": "usd", "payment": {"type": "payment_intent", "payment_intent": "pi_fixture"}}], "has_more": False},
            "payment_intents/pi_fixture": {"livemode": False, "status": "succeeded", "currency": "usd",
                "amount_received": 700, "latest_charge": "ch_fixture"},
            "charges/ch_fixture": {"livemode": False, "status": "succeeded", "paid": True, "captured": True,
                "currency": "usd", "amount": 700, "amount_captured": 700, "amount_refunded": 0,
                "payment_intent": "pi_fixture"},
        }
        if mutation:
            mutation(payloads)

        def transport(request, timeout):
            self.assertEqual(request.get_header("Stripe-version"), spike.API_VERSION)
            self.assertEqual(urllib.parse.urlsplit(request.full_url).netloc, "api.stripe.com")
            path = urllib.parse.urlsplit(request.full_url).path.removeprefix("/v1/")
            return Response(deepcopy(payloads[path]))
        return transport

    def test_missing_and_live_keys_never_contact_transport(self):
        code, result, network = self.invoke(credentials=False)
        self.assertEqual((code, result["winner"], result["reason"]), (2, "inconclusive", "credential_required"))
        network.assert_not_called()
        self.credentials.write_text(json.dumps({"STRIPE_SECRET_KEY": "sk_live_SyntheticForbidden"}))
        code, result, network = self.invoke()
        self.assertEqual((code, result["reason"]), (2, "credential_invalid"))
        network.assert_not_called()

    def test_credentials_are_json_data_not_shell_programs(self):
        self.credentials.write_text('STRIPE_SECRET_KEY=$(touch unexpectedly-executed)')
        code, result, network = self.invoke()
        self.assertEqual((code, result["reason"]), (2, "credential_invalid"))
        self.assertFalse(Path("unexpectedly-executed").exists())
        network.assert_not_called()

    def test_errors_and_reflected_secrets_do_not_authorize_fallback(self):
        for status, kind, message in (
            (401, "authentication_error", "Managed Payments does not support metered prices " + KEY),
            (403, "invalid_request_error", "Managed Payments does not support metered prices"),
            (429, "rate_limit_error", "Managed Payments does not support metered prices"),
            (500, "api_error", "Managed Payments does not support metered prices"),
            (400, "invalid_request_error", "Test clock customers cannot use Managed Payments metered prices"),
            (400, "invalid_request_error", "Unknown parameter: managed_payments " + KEY),
        ):
            with self.subTest(status=status, kind=kind):
                self.partial_state()
                error = urllib.error.HTTPError("https://api.stripe.com/v1/checkout/sessions", status, KEY,
                    {"Request-Id": "req_refusal"}, io.BytesIO(json.dumps({"error": {
                        "type": kind, "message": message, "param": KEY, "code": KEY}}).encode()))
                code, result, network = self.invoke(transport=error)
                self.assertEqual((code, result["winner"], result["compatibility_error"]), (2, "inconclusive", None))
                self.assertEqual(network.call_count, 1)
                _, fallback, network = self.invoke("fallback")
                self.assertEqual(fallback["reason"], "fallback_not_authorized")
                network.assert_not_called()

    def test_definitive_refusal_records_only_normalized_metadata(self):
        self.partial_state()
        error = urllib.error.HTTPError("https://api.stripe.com/v1/checkout/sessions", 400, KEY,
            {"Request-Id": "req_refusal"}, io.BytesIO(json.dumps({"error": {
                "type": "invalid_request_error", "message": "Managed Payments does not support metered prices. " + KEY}}).encode()))
        code, result, _ = self.invoke(transport=error)
        self.assertEqual((code, result["winner"], result["reason"]), (2, "inconclusive", "definitive_incompatibility"))
        self.assertEqual(result["compatibility_error"], {
            "kind": "metered_managed_payments_unsupported", "status": 400, "request_id": "req_refusal"})
        self.assertNotIn("fallback_session", result["object_ids"])

    def test_network_failure_is_inconclusive_without_fallback(self):
        for error in (urllib.error.URLError(KEY), http.client.BadStatusLine(KEY)):
            with self.subTest(error=type(error).__name__):
                self.partial_state()
                code, result, _ = self.invoke(transport=error)
                self.assertEqual((code, result["winner"], result["reason"], result["compatibility_error"]),
                                 (2, "inconclusive", "network_error", None))

    def test_success_metadata_cannot_reflect_secret_in_id_or_header(self):
        for field in ("id", "header"):
            with self.subTest(field=field):
                self.partial_state()
                def response(request, timeout):
                    value = Response({"id": "cs_test_" + KEY if field == "id" else "cs_test_fixture", "livemode": False})
                    if field == "header":
                        value.headers["Request-Id"] = "req_" + KEY
                    return value
                code, result, _ = self.invoke(transport=response)
                self.assertEqual((code, result["winner"], result["reason"]), (2, "inconclusive", "response_invalid"))

    def test_created_session_still_requires_human_checkout(self):
        state = self.partial_state()
        def response(request, timeout):
            params = urllib.parse.parse_qs(request.data.decode())
            # Independent Stripe protocol boundary: metered items omit quantity.
            self.assertEqual(params["managed_payments[enabled]"], ["true"])
            self.assertEqual(params["mode"], ["subscription"])
            self.assertEqual(params["line_items[0][price]"], [state["object_ids"]["price"]])
            self.assertNotIn("line_items[0][quantity]", params)
            return Response({"id": "cs_test_fixture", "livemode": False})
        code, result, _ = self.invoke(transport=response)
        self.assertEqual((code, result["winner"], result["reason"]), (2, "inconclusive", "checkout_pending"))
        self.assertTrue(result["evidence"]["metered_checkout_accepted"])
        self.assertFalse(result["evidence"]["checkout_complete"])

    def test_only_matching_paid_cycle_and_captured_charge_can_pass(self):
        mutations = (
            ("valid", None, 0),
            ("checkout_open", lambda p: p["checkout/sessions/cs_test_fixture"].update(status="open"), 2),
            ("wrong_period", lambda p: p["invoices/in_fixture/lines"]["data"][0].update(period={"start": 1800000000, "end": 1802678460}), 2),
            ("wrong_price", lambda p: p["invoices/in_fixture/lines"]["data"][0]["pricing"]["price_details"].update(price="price_other"), 2),
            ("wrong_quantity", lambda p: p["invoices/in_fixture/lines"]["data"][0].update(quantity=6), 2),
            ("initial_invoice", lambda p: p["invoices"]["data"][0].update(billing_reason="subscription_create"), 2),
            ("invoice_unpaid", lambda p: p["invoices"]["data"][0].update(status="open", amount_paid=0), 2),
            ("wrong_invoice", lambda p: p["invoice_payments"]["data"][0].update(invoice="in_other"), 2),
            ("no_intent", lambda p: p["invoice_payments"].update(data=[]), 2),
            ("uncaptured", lambda p: p["charges/ch_fixture"].update(captured=False), 2),
            ("refunded", lambda p: p["charges/ch_fixture"].update(amount_refunded=700), 2),
            ("wrong_charge", lambda p: p["charges/ch_fixture"].update(payment_intent="pi_other"), 2),
            ("async_usage", lambda p: p["billing/meters/mtr_fixture/event_summaries"]["data"][0].update(aggregated_value=0), 2),
        )
        for name, mutation, expected in mutations:
            with self.subTest(name=name):
                state = self.billing_state()
                code, result, _ = self.invoke("verify", self.billing_transport(state, mutation))
                self.assertEqual(code, expected)
                self.assertEqual(result["winner"], "metered_overage" if expected == 0 else "inconclusive")
                if code == 0:
                    self.assertEqual(result["object_ids"]["charge"], "ch_fixture")
                    self.assertEqual(result["evidence"]["invoice_quantity"], 7)

    def test_usage_is_idempotent_and_clock_waits_for_aggregation(self):
        state = self.billing_state()
        state["evidence"].update(meter_event_accepted=False, usage_aggregated=False,
                                  cycle_advanced=False, settlement_advanced=False)
        self.output.write_text(json.dumps(state))
        base = self.billing_transport(state, lambda p: p["test_helpers/test_clocks/clock_fixture"].update(
            frozen_time=state["evidence"]["period_start"]))
        writes = []

        def transport(request, timeout):
            if request.method == "POST":
                writes.append(request)
                params = urllib.parse.parse_qs(request.data.decode())
                self.assertEqual(params["payload[value]"], ["7"])
                self.assertEqual(params["payload[stripe_customer_id]"], [state["object_ids"]["customer"]])
                self.assertEqual(params["identifier"], [state["run_id"] + "-usage"])
                return Response({"livemode": False})
            return base(request, timeout)

        code, result, _ = self.invoke("usage", transport)
        self.assertEqual(code, 2)
        self.assertTrue(result["evidence"]["meter_event_accepted"])
        self.invoke("usage", transport)
        self.assertEqual(len(writes), 1)
        delayed = self.billing_transport(state, lambda p: p["billing/meters/mtr_fixture/event_summaries"]["data"][0].update(aggregated_value=0))
        _, result, network = self.invoke("advance", delayed)
        self.assertEqual(result["reason"], "aggregation_pending")
        self.assertFalse(result["evidence"]["cycle_advanced"])
        self.assertTrue(all(call.args[0].method == "GET" for call in network.call_args_list))

    def test_fallback_requires_actual_oneoff_checkout_and_capture(self):
        state = self.partial_state()
        state["compatibility_error"] = {
            "kind": "metered_managed_payments_unsupported", "status": 400, "request_id": "req_refusal"}
        self.output.write_text(json.dumps(state))
        posted = []

        def create(request, timeout):
            params = urllib.parse.parse_qs(request.data.decode())
            posted.append(params)
            if request.full_url.endswith("/prices"):
                self.assertEqual(params["unit_amount"], ["700"])
                self.assertNotIn("recurring[usage_type]", params)
                return Response({"id": "price_oneoff", "livemode": False})
            self.assertEqual(params["mode"], ["payment"])
            self.assertEqual(params["managed_payments[enabled]"], ["true"])
            return Response({"id": "cs_test_oneoff", "livemode": False})

        code, result, _ = self.invoke("fallback", create)
        self.assertEqual((code, result["winner"], len(posted)), (2, "inconclusive", 2))
        prepared = deepcopy(result)
        session = {"id": "cs_test_oneoff", "livemode": False, "mode": "payment",
                   "metadata": {"aidash_spike": state["run_id"]}, "managed_payments": {"enabled": True},
                   "status": "complete", "payment_status": "paid", "currency": "usd",
                   "amount_total": 700, "payment_intent": "pi_fixture"}
        # Reuse only the actual charge API fixtures; fallback has its own Checkout contract.
        base = self.billing_transport(self.billing_state())

        for status in ("unpaid", "paid"):
            self.output.write_text(json.dumps(prepared))
            session["payment_status"] = status

            def verify(request, timeout):
                path = urllib.parse.urlsplit(request.full_url).path
                if path == "/v1/checkout/sessions/cs_test_oneoff":
                    return Response(session)
                if path == "/v1/checkout/sessions/cs_test_oneoff/line_items":
                    return Response({"has_more": False, "data": [{"price": {"id": "price_oneoff"}, "quantity": 1}]})
                return base(request, timeout)

            code, result, _ = self.invoke("verify", verify)
            self.assertEqual(result["winner"], "usage_credits" if status == "paid" else "inconclusive")
            self.assertEqual(code, 0 if status == "paid" else 2)

    def test_old_uncertain_writes_are_not_retried_after_idempotency_window(self):
        state = self.partial_state()
        state["created_at"] -= 24 * 3600
        self.output.write_text(json.dumps(state))
        code, result, network = self.invoke()
        self.assertEqual((code, result["reason"]), (2, "retry_window_expired"))
        network.assert_not_called()

    def test_arbitrary_state_fields_are_not_reemitted(self):
        state = self.partial_state()
        state["unexpected"] = KEY
        self.output.write_text(json.dumps(state))
        code, result, network = self.invoke()
        self.assertEqual((code, result["reason"]), (2, "state_invalid"))
        self.assertNotIn("unexpected", result)
        network.assert_not_called()


if __name__ == "__main__":
    unittest.main()
