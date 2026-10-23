#!/usr/bin/env python3
"""Resumable, sandbox-only Managed Payments compatibility experiment.

No SDK, ambient credentials, shell evaluation, raw-response logging, or automatic
model choice on HTTP errors. See docs/operations/stripe-billing.md before use.
"""

import argparse
import http.client
import json
import os
from pathlib import Path
import re
import stat
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid
import webbrowser

API_VERSION = "2026-09-30.endive"
QUANTITY = 7
AMOUNT = 700  # Sandbox fixture only: seven units at USD 1, tax inclusive.
PHASES = ("prepare", "checkout", "usage", "aggregate", "advance", "settle", "verify", "fallback")
ID_PREFIXES = {
    "clock": "clock_", "customer": "cus_", "product": "prod_", "meter": "mtr_",
    "price": "price_", "session": "cs_test_", "subscription": "sub_",
    "fallback_price": "price_", "fallback_session": "cs_test_", "invoice": "in_",
    "payment_intent": "pi_", "charge": "ch_",
}
EVIDENCE = {
    "checkout_complete": False, "metered_checkout_accepted": False,
    "meter_event_accepted": False, "usage_aggregated": False,
    "cycle_advanced": False, "settlement_advanced": False,
    "fallback_checkout_complete": False, "charge_verified": False,
    "period_start": 0, "period_end": 0, "usage_timestamp": 0,
    "aggregated_quantity": 0, "invoice_quantity": 0, "invoice_amount": 0,
    "charge_amount": 0, "subscription_active": False,
}
REASONS = {
    "pending", "credential_required", "credential_invalid", "state_invalid",
    "local_io_error", "network_error", "stripe_error", "response_invalid",
    "checkout_pending", "usage_pending", "aggregation_pending", "clock_pending",
    "cycle_pending", "charge_pending", "fallback_not_authorized", "verified",
    "definitive_incompatibility", "retry_window_expired",
}
STEPS = {
    "clock", "customer", "product", "meter", "price", "session", "fallback_price",
    "fallback_session", "checkout", "subscription", "clock_check",
    "usage", "aggregate", "advance", "settle", "invoice", "lines", "payments",
    "payment_intent", "charge", "fallback_lines",
}


class Pending(Exception):
    def __init__(self, reason):
        self.reason = reason


class StripeFailure(Pending):
    def __init__(self, status, request_id, definitive=False):
        super().__init__("definitive_incompatibility" if definitive else "stripe_error")
        self.status = status
        self.request_id = request_id
        self.definitive = definitive


def identifier(value, prefix):
    if isinstance(value, dict):
        value = value.get("id")
    if (not isinstance(value, str) or len(value) > 250
            or not re.fullmatch(re.escape(prefix) + r"[A-Za-z0-9_]+", value)
            or re.search(r"(?:sk|rk|pk)_(?:test|live)_", value)):
        raise Pending("response_invalid")
    return value


def number(value):
    if type(value) is not int or not 0 <= value <= 10**12:
        raise Pending("response_invalid")
    return value


def request_id(value):
    if value is None:
        return None
    # Request IDs are scalar headers/journal values, never expanded API objects.
    if not isinstance(value, str):
        raise Pending("state_invalid")
    return identifier(value, "req_")


def sandbox(obj):
    if not isinstance(obj, dict) or obj.get("livemode") is not False:
        raise Pending("response_invalid")
    return obj


def definitive_metered_refusal(status, payload, step):
    """Only an explicit semantic refusal at metered Checkout permits fallback.

    Never persist error messages. Unknown wording needs operator investigation,
    not a widened classifier or a model decision inferred from an HTTP status.
    """
    error = payload.get("error", {}) if isinstance(payload, dict) else {}
    if status != 400 or step != "session" or error.get("type") != "invalid_request_error":
        return False
    message = error.get("message", "")
    if not isinstance(message, str):
        return False
    message = " ".join(message.lower().split())
    # Exclude setup/control failures even when their text mentions metering.
    if any(word in message for word in ("test clock", "permission", "tax code", "country", "activate")):
        return False
    metering = r"(?:metered (?:prices?|billing|subscriptions?)|usage-based (?:billing|prices?|subscriptions?))"
    refusal = r"(?:is |are )?(?:not supported|unsupported|incompatible)"
    return bool(re.search(metering + r"[^.!?]{0,100}" + refusal + r"[^.!?]{0,100}managed payments", message)
                or re.search(r"managed payments (?:does not|doesn't|cannot) support[^.!?]{0,80}" + metering, message)
                or re.search(r"managed payments[^.!?]{0,80}" + refusal + r"[^.!?]{0,80}" + metering, message))


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


class Client:
    def __init__(self, key, state, save):
        self.key, self.state, self.save = key, state, save
        # Do not send credentials through environment-configured HTTP proxies.
        self.opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())

    def request(self, method, path, step, params=None):
        params = params or {}
        headers = {"Authorization": "Bearer " + self.key, "Stripe-Version": API_VERSION}
        data = urllib.parse.urlencode(params).encode()
        url = "https://api.stripe.com/v1/" + path
        if method == "GET":
            url += ("?" + data.decode()) if data else ""
            data = None
        else:
            # Stripe can prune idempotency keys after 24 hours. Never silently
            # duplicate a timed-out creation/event when resuming an old run.
            if int(time.time()) - self.state["created_at"] >= 23 * 3600:
                raise Pending("retry_window_expired")
            if not self.state["write_attempted"]:
                # A lost first response leaves no object/request ID. Persist the
                # attempt before transport so resuming cannot reset its identity.
                self.state["write_attempted"] = True
                self.save()
            headers["Idempotency-Key"] = self.state["run_id"] + "-" + step
        req = urllib.request.Request(url, data=data, headers=headers, method=method)
        try:
            with self.opener.open(req, timeout=30) as response:
                rid = request_id(response.headers.get("Request-Id"))
                raw = response.read(2_000_001)
                if len(raw) > 2_000_000:
                    raise Pending("response_invalid")
                payload = json.loads(raw)
                if not isinstance(payload, dict):
                    raise Pending("response_invalid")
        except urllib.error.HTTPError as exc:
            rid = request_id(exc.headers.get("Request-Id"))
            try:
                raw = exc.read(2_000_001)
                payload = json.loads(raw) if len(raw) <= 2_000_000 else {}
            except (ValueError, OSError, http.client.HTTPException, RecursionError):
                payload = {}
            definitive = definitive_metered_refusal(exc.code, payload, step)
            self.record(step, exc.code, rid)
            raise StripeFailure(exc.code, rid, definitive) from None
        except (urllib.error.URLError, TimeoutError, OSError, http.client.HTTPException):
            raise Pending("network_error") from None
        except (ValueError, UnicodeError, RecursionError):
            raise Pending("response_invalid") from None
        self.record(step, 200, rid)
        return payload

    def record(self, step, status, rid):
        self.state["request_id"] = rid
        self.state["requests"].append({"step": step, "status": status, "request_id": rid})
        self.save()

    def items(self, path, step, params=None):
        query = dict(params or {}, limit=100)
        for _ in range(20):
            page = self.request("GET", path, step, query)
            if not isinstance(page.get("data"), list):
                raise Pending("response_invalid")
            yield from page["data"]
            if page.get("has_more") is False:
                return
            if not page["data"]:
                break
            # Cursor stays in memory; never follows Stripe-supplied URLs.
            query["starting_after"] = identifier(page["data"][-1], "")
        raise Pending("response_invalid")


def credentials(path):
    if path is None:
        raise Pending("credential_required")
    try:
        info = path.stat()
        if not stat.S_ISREG(info.st_mode) or info.st_mode & 0o077 or info.st_size > 8192:
            raise Pending("credential_invalid")
        obj = json.loads(path.read_text())
        if not isinstance(obj, dict) or set(obj) != {"STRIPE_SECRET_KEY"}:
            raise Pending("credential_invalid")
        key = obj["STRIPE_SECRET_KEY"]
        if not isinstance(key, str) or not re.fullmatch(r"(?:sk|rk)_test_[A-Za-z0-9]+", key):
            raise Pending("credential_invalid")
        return key
    except (OSError, ValueError, UnicodeError, RecursionError):
        raise Pending("credential_invalid") from None


def fresh_state():
    return {"schema": 1, "winner": "inconclusive", "api_version": API_VERSION,
            "run_id": uuid.uuid4().hex, "created_at": int(time.time()) // 60 * 60,
            "write_attempted": False, "request_id": None, "object_ids": {}, "compatibility_error": None,
            "reason": "pending", "requests": [], "evidence": dict(EVIDENCE)}


def validate_state(state):
    """Reject arbitrary state fields before re-emitting any local input."""
    template = fresh_state()
    if isinstance(state, dict) and set(state) == set(template) - {"write_attempted"}:
        # Earlier journals cannot distinguish an unsent run from a lost first
        # response. Preserve their original identity/window conservatively.
        state["write_attempted"] = True
    if not isinstance(state, dict) or set(state) != set(template):
        raise Pending("state_invalid")
    if (state["schema"] != 1 or state["api_version"] != API_VERSION
            or state["winner"] not in ("inconclusive", "metered_overage", "usage_credits")
            or state["reason"] not in REASONS
            or not re.fullmatch(r"[a-f0-9]{32}", state["run_id"])):
        raise Pending("state_invalid")
    number(state["created_at"])
    if type(state["write_attempted"]) is not bool:
        raise Pending("state_invalid")
    request_id(state["request_id"])
    ids = state["object_ids"]
    if not isinstance(ids, dict) or not set(ids) <= set(ID_PREFIXES):
        raise Pending("state_invalid")
    for name, value in ids.items():
        # API responses may expand IDs into objects; persisted journals may not.
        if not isinstance(value, str):
            raise Pending("state_invalid")
        identifier(value, ID_PREFIXES[name])
    evidence = state["evidence"]
    if not isinstance(evidence, dict) or set(evidence) != set(EVIDENCE):
        raise Pending("state_invalid")
    for name, default in EVIDENCE.items():
        if type(default) is bool:
            if type(evidence[name]) is not bool:
                raise Pending("state_invalid")
        else:
            number(evidence[name])
    records = state["requests"]
    if not isinstance(records, list) or len(records) > 10000:
        raise Pending("state_invalid")
    for record in records:
        if (not isinstance(record, dict) or set(record) != {"step", "status", "request_id"}
                or record["step"] not in STEPS or type(record["status"]) is not int
                or not 200 <= record["status"] <= 599):
            raise Pending("state_invalid")
        request_id(record["request_id"])
    error = state["compatibility_error"]
    if error is not None:
        if (not isinstance(error, dict) or set(error) != {"kind", "status", "request_id"}
                or error["kind"] != "metered_managed_payments_unsupported" or error["status"] != 400):
            raise Pending("state_invalid")
        request_id(error["request_id"])
    return state


class Experiment:
    def __init__(self, client, state, save):
        self.api, self.state, self.save = client, state, save
        self.ids, self.e = state["object_ids"], state["evidence"]

    def create(self, name, path, params):
        if name not in self.ids:
            obj = sandbox(self.api.request("POST", path, name, params))
            self.ids[name] = identifier(obj, ID_PREFIXES[name])
            self.save()
        return self.ids[name]

    def prepare(self, fallback=False):
        run = self.state["run_id"]
        if fallback and self.state["compatibility_error"] is None:
            raise Pending("fallback_not_authorized")
        product = self.create("product", "products", {
            "name": "Aidash disposable sandbox spike " + run, "tax_code": "txcd_10103001",
            "metadata[aidash_spike]": run})
        if fallback:
            price_name, session_name = "fallback_price", "fallback_session"
            price = self.create(price_name, "prices", {
                "product": product, "currency": "usd", "unit_amount": AMOUNT,
                "tax_behavior": "inclusive", "metadata[aidash_spike]": run})
        else:
            clock = self.create("clock", "test_helpers/test_clocks", {
                "frozen_time": self.state["created_at"], "name": "aidash-spike-" + run})
            customer = self.create("customer", "customers", {
                "test_clock": clock, "metadata[aidash_spike]": run})
            meter = self.create("meter", "billing/meters", {
                "display_name": "Aidash sandbox units " + run, "event_name": "aidash_spike_" + run,
                "default_aggregation[formula]": "sum", "value_settings[event_payload_key]": "value",
                "customer_mapping[type]": "by_id", "customer_mapping[event_payload_key]": "stripe_customer_id"})
            price_name, session_name = "price", "session"
            price = self.create(price_name, "prices", {
                "product": product, "currency": "usd", "unit_amount": 100,
                "tax_behavior": "inclusive", "recurring[interval]": "month",
                "recurring[usage_type]": "metered", "recurring[meter]": meter,
                "metadata[aidash_spike]": run})
        params = {"mode": "payment" if fallback else "subscription", "managed_payments[enabled]": "true",
                  "line_items[0][price]": price, "metadata[aidash_spike]": run,
                  "success_url": "https://localhost:4242/success"}
        if fallback:
            params["line_items[0][quantity]"] = 1
        else:
            params["customer"] = customer
            params["subscription_data[metadata][aidash_spike]"] = run
            # A metered Checkout line must not have quantity.
        self.create(session_name, "checkout/sessions", params)
        if not fallback:
            self.e["metered_checkout_accepted"] = True
        self.save()
        raise Pending("checkout_pending")

    def session(self, fallback=False, open_browser=False):
        name = "fallback_session" if fallback else "session"
        if name not in self.ids:
            raise Pending("checkout_pending")
        obj = sandbox(self.api.request("GET", "checkout/sessions/" + self.ids[name], "checkout"))
        if (identifier(obj, "cs_test_") != self.ids[name]
                or obj.get("metadata", {}).get("aidash_spike") != self.state["run_id"]
                or obj.get("managed_payments", {}).get("enabled") is not True
                or obj.get("mode") != ("payment" if fallback else "subscription")):
            raise Pending("response_invalid")
        if obj.get("status") != "complete":
            if open_browser:
                url = obj.get("url", "")
                parsed = urllib.parse.urlsplit(url)
                if (parsed.scheme != "https" or parsed.hostname != "checkout.stripe.com"
                        or parsed.username or parsed.password or parsed.port):
                    raise Pending("response_invalid")
                webbrowser.open(url)  # Never emit or persist the bearer-like Checkout URL.
            raise Pending("checkout_pending")
        self.e["fallback_checkout_complete" if fallback else "checkout_complete"] = True
        self.save()
        return obj

    def subscription(self):
        session = self.session()
        sid = identifier(session.get("subscription"), "sub_")
        if identifier(session.get("customer"), "cus_") != self.ids["customer"]:
            raise Pending("response_invalid")
        obj = sandbox(self.api.request("GET", "subscriptions/" + sid, "subscription"))
        if (obj.get("metadata", {}).get("aidash_spike") != self.state["run_id"]
                or identifier(obj.get("customer"), "cus_") != self.ids["customer"]
                or identifier(obj.get("test_clock"), "clock_") != self.ids["clock"]):
            raise Pending("response_invalid")
        items = obj.get("items", {})
        if items.get("has_more") is not False or len(items.get("data", [])) != 1:
            raise Pending("response_invalid")
        item = items["data"][0]
        if identifier(item.get("price"), "price_") != self.ids["price"]:
            raise Pending("response_invalid")
        self.ids["subscription"] = sid
        self.e["subscription_active"] = obj.get("status") == "active"
        if not self.e["subscription_active"]:
            raise Pending("cycle_pending")
        if not self.e["period_end"]:
            self.e["period_start"] = number(item.get("current_period_start"))
            self.e["period_end"] = number(item.get("current_period_end"))
        self.save()
        return obj

    def clock(self):
        obj = self.api.request("GET", "test_helpers/test_clocks/" + self.ids["clock"], "clock_check")
        if (obj.get("livemode") is not False or obj.get("name") != "aidash-spike-" + self.state["run_id"]):
            raise Pending("response_invalid")
        if obj.get("status") != "ready":
            raise Pending("clock_pending")
        return number(obj.get("frozen_time"))

    def usage(self):
        self.subscription()
        if self.e["meter_event_accepted"]:
            return
        frozen = self.clock()
        if not self.e["period_start"] <= frozen < self.e["period_end"]:
            raise Pending("cycle_pending")
        self.e["usage_timestamp"] = frozen
        self.save()
        sandbox(self.api.request("POST", "billing/meter_events", "usage", {
            "event_name": "aidash_spike_" + self.state["run_id"],
            "identifier": self.state["run_id"] + "-usage", "timestamp": frozen,
            "payload[stripe_customer_id]": self.ids["customer"], "payload[value]": QUANTITY}))
        self.e["meter_event_accepted"] = True
        self.save()

    def aggregate(self):
        if not self.e["meter_event_accepted"]:
            raise Pending("usage_pending")
        start = self.e["period_start"] // 60 * 60
        end = (self.e["period_end"] + 59) // 60 * 60
        rows = self.api.items("billing/meters/" + self.ids["meter"] + "/event_summaries", "aggregate", {
            "customer": self.ids["customer"], "start_time": start, "end_time": end})
        total = 0
        for row in rows:
            sandbox(row)
            if identifier(row.get("meter"), "mtr_") != self.ids["meter"]:
                raise Pending("response_invalid")
            value = row.get("aggregated_value")
            if type(value) not in (int, float) or value < 0 or value != int(value):
                raise Pending("response_invalid")
            total += int(value)
        self.e["aggregated_quantity"] = number(total)
        self.e["usage_aggregated"] = total == QUANTITY
        self.save()
        if not self.e["usage_aggregated"]:
            raise Pending("aggregation_pending")

    def advance(self, settle=False):
        self.subscription()
        self.aggregate()
        if settle and not self.e["cycle_advanced"]:
            raise Pending("cycle_pending")
        target = self.e["period_end"] + (7200 if settle else 0)
        frozen = self.clock()
        if frozen < target:
            self.api.request("POST", "test_helpers/test_clocks/" + self.ids["clock"] + "/advance",
                             "settle" if settle else "advance", {"frozen_time": target})
            raise Pending("clock_pending")
        self.e["settlement_advanced" if settle else "cycle_advanced"] = True
        self.save()

    def charge(self, intent_id, amount):
        intent_id = identifier(intent_id, "pi_")
        intent = sandbox(self.api.request("GET", "payment_intents/" + intent_id, "payment_intent"))
        if (intent.get("status") != "succeeded" or intent.get("currency") != "usd"
                or intent.get("amount_received") != amount):
            raise Pending("charge_pending")
        cid = identifier(intent.get("latest_charge"), "ch_")
        charge = sandbox(self.api.request("GET", "charges/" + cid, "charge"))
        if (charge.get("status") != "succeeded" or charge.get("paid") is not True
                or charge.get("captured") is not True or charge.get("currency") != "usd"
                or charge.get("amount") != amount or charge.get("amount_captured") != amount
                or charge.get("amount_refunded") != 0
                or identifier(charge.get("payment_intent"), "pi_") != intent_id):
            raise Pending("charge_pending")
        self.ids.update(payment_intent=intent_id, charge=cid)
        self.e.update(charge_verified=True, charge_amount=amount)

    def verify(self, fallback=False):
        if fallback:
            if self.state["compatibility_error"] is None:
                raise Pending("fallback_not_authorized")
            session = self.session(True)
            rows = list(self.api.items("checkout/sessions/" + self.ids["fallback_session"] + "/line_items", "fallback_lines"))
            if (len(rows) != 1 or identifier(rows[0].get("price"), "price_") != self.ids["fallback_price"]
                    or rows[0].get("quantity") != 1 or session.get("amount_total") != AMOUNT
                    or session.get("currency") != "usd" or session.get("payment_status") != "paid"):
                raise Pending("charge_pending")
            self.charge(session.get("payment_intent"), AMOUNT)
            self.state["winner"] = "usage_credits"
        else:
            self.subscription()
            self.aggregate()
            if not self.e["cycle_advanced"] or not self.e["settlement_advanced"]:
                raise Pending("cycle_pending")
            self.clock()
            invoices = self.api.items("invoices", "invoice", {"subscription": self.ids["subscription"], "customer": self.ids["customer"]})
            for invoice in invoices:
                sandbox(invoice)
                if (identifier(invoice.get("customer"), "cus_") != self.ids["customer"]
                        or identifier(invoice.get("test_clock"), "clock_") != self.ids["clock"]
                        or identifier(invoice.get("parent", {}).get("subscription_details", {}).get("subscription"), "sub_") != self.ids["subscription"]):
                    raise Pending("response_invalid")
                if invoice.get("billing_reason") != "subscription_cycle":
                    continue
                iid = identifier(invoice, "in_")
                rows = list(self.api.items("invoices/" + iid + "/lines", "lines"))
                if len(rows) != 1:
                    continue
                line = sandbox(rows[0])
                if (identifier(line.get("pricing", {}).get("price_details", {}).get("price"), "price_") != self.ids["price"]
                        or line.get("period") != {"start": self.e["period_start"], "end": self.e["period_end"]}
                        or line.get("quantity") != QUANTITY or line.get("amount") != AMOUNT):
                    continue
                if (invoice.get("status") != "paid" or invoice.get("currency") != "usd"
                        or invoice.get("amount_paid") != AMOUNT or invoice.get("amount_remaining") != 0
                        or invoice.get("total") != AMOUNT):
                    continue
                payments = list(self.api.items("invoice_payments", "payments", {"invoice": iid, "status": "paid"}))
                for payment in payments:
                    sandbox(payment)
                    if (identifier(payment.get("invoice"), "in_") == iid and payment.get("status") == "paid"
                            and payment.get("currency") == "usd" and payment.get("amount_paid") == AMOUNT
                            and payment.get("payment", {}).get("type") == "payment_intent"):
                        self.charge(payment["payment"].get("payment_intent"), AMOUNT)
                        self.ids["invoice"] = iid
                        self.e.update(invoice_quantity=QUANTITY, invoice_amount=AMOUNT)
                        self.state["winner"] = "metered_overage"
                        self.save()
                        return
            raise Pending("charge_pending")
        self.save()


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--credential-file", type=Path, help="Explicit mode-0600 JSON file; never sourced")
    parser.add_argument("--output", type=Path, default=Path("notes/stripe-spike.json"))
    parser.add_argument("--phase", choices=PHASES, default="prepare")
    parser.add_argument("--open-checkout", action="store_true", help="Open the hosted sandbox Checkout URL without printing it")
    args = parser.parse_args(argv)
    state = fresh_state()
    output = args.output.resolve()
    notes = (Path.cwd() / "notes").resolve()
    try:
        if not output.is_relative_to(notes) or output == notes or output.is_symlink():
            raise Pending("state_invalid")
        if output.exists():
            if output.stat().st_size > 2_000_000:
                raise Pending("state_invalid")
            state = validate_state(json.loads(output.read_text()))
        # Each run must earn its winner through current Stripe verification.
        state["winner"] = "inconclusive"
        state["reason"] = "pending"

        def save():
            validate_state(state)
            body = json.dumps(state, indent=2, sort_keys=True) + "\n"
            output.parent.mkdir(parents=True, exist_ok=True)
            temporary = output.with_suffix(output.suffix + ".tmp")
            fd = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
            with os.fdopen(fd, "w") as handle:
                handle.write(body)
            os.replace(temporary, output)

        key = credentials(args.credential_file)
        if not state["write_attempted"] and not state["object_ids"] and not state["requests"]:
            # A known-never-sent journal can begin its write window now. Keep
            # its run ID; once any write is attempted this time is immutable.
            state["created_at"] = int(time.time()) // 60 * 60
        # A loaded state can never reflect this key through allowed ID fields.
        if key in json.dumps(state):
            raise Pending("state_invalid")
        save()
        api = Client(key, state, save)
        experiment = Experiment(api, state, save)
        fallback = state["compatibility_error"] is not None
        try:
            if args.phase == "prepare":
                experiment.prepare()
            elif args.phase == "fallback":
                experiment.prepare(True)
            elif args.phase == "checkout":
                experiment.session(fallback, args.open_checkout)
            elif args.phase == "usage":
                experiment.usage()
            elif args.phase == "aggregate":
                experiment.aggregate()
            elif args.phase in ("advance", "settle"):
                experiment.advance(args.phase == "settle")
            elif args.phase == "verify":
                experiment.verify(fallback)
        except StripeFailure as exc:
            if exc.definitive:
                state["compatibility_error"] = {"kind": "metered_managed_payments_unsupported",
                                                "status": exc.status, "request_id": exc.request_id}
            raise
        state["reason"] = "verified" if state["winner"] != "inconclusive" else "pending"
    except Pending as exc:
        # Invalid local state is never re-emitted; exception text is never used.
        if exc.reason in ("state_invalid", "response_invalid"):
            try:
                validate_state(state)
            except (Pending, TypeError, KeyError):
                state = fresh_state()
        state["winner"] = "inconclusive"
        state["reason"] = exc.reason
    except Exception:
        # Never expose exception text: even protocol/parser errors can reflect
        # a remote body. Preserve any already-validated resource inventory.
        try:
            validate_state(state)
        except (Pending, TypeError, KeyError, ValueError):
            state = fresh_state()
        state["winner"] = "inconclusive"
        state["reason"] = "local_io_error"
    try:
        validate_state(state)
        body = json.dumps(state, indent=2, sort_keys=True) + "\n"
        # No-credential runs still produce a reviewable, inconclusive artifact.
        if output.is_relative_to(notes) and output != notes:
            output.parent.mkdir(parents=True, exist_ok=True)
            temporary = output.with_suffix(output.suffix + ".final.tmp")
            fd = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
            with os.fdopen(fd, "w") as handle:
                handle.write(body)
            os.replace(temporary, output)
        print(body, end="")
    except (OSError, Pending, TypeError, ValueError):
        print(json.dumps({"winner": "inconclusive", "api_version": API_VERSION,
                          "request_id": None, "object_ids": {}, "compatibility_error": None,
                          "reason": "local_io_error"}))
        return 2
    return 0 if state["winner"] != "inconclusive" else 2


if __name__ == "__main__":
    sys.exit(main())
