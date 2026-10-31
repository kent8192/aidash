# SPDX-FileCopyrightText: 2026 kent8192
# SPDX-License-Identifier: BUSL-1.1
# See enterprise/LICENSE.

"""Notification protocol, credential boundary and durable delivery contracts."""

import io
import json
import os
import ssl
import sys
import unittest
import urllib.request
from copy import deepcopy
from pathlib import Path
from unittest.mock import Mock, patch
from urllib.error import HTTPError, URLError
from urllib.parse import parse_qs, urlparse

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "control"))
from notifications import (
    CloudflareSender,
    Refused,
    SendFailure,
    drain,
    message,
    preflight,
)

NOW = 1791766800
CONFIG = {"deployment_tier": "nonproduction"}


def receipt(identity="develop", **changes):
    value = {
        "protocol": "aidash-deployment-notification/1",
        "event_id": "a" * 64,
        "environment": identity,
        "incarnation": "b" * 12,
        "generation": 3,
        "source_sha": "c" * 40,
        "trusted_sha": "d" * 40,
        "status": "ready",
        "hostname": "e" * 32 + ".preview.aidash.run",
        "run_id": "1234",
        "occurred_at": NOW,
        "images": {
            "app": "us-central1-docker.pkg.dev/aidash-test/aidash/app@sha256:"
            + "f" * 64
        },
    }
    value.update(changes)
    return value


def mail_payload():
    with patch("notifications.time.time", return_value=NOW):
        return message(receipt(), CONFIG)


class MemoryStore:
    """Independent GCS CAS and prefix-list transport used by the consumer."""

    bucket = "notifications-test"

    def __init__(self, receipts):
        self.objects = {
            f"notifications/{item['event_id']}.json": (deepcopy(item), "1")
            for item in receipts
        }
        self.writes = []

    def read(self, key):
        return deepcopy(self.objects.get(key, (None, "0")))

    def write(self, key, value, generation):
        previous = self.objects.get(key, (None, "0"))[1]
        if generation != previous:
            raise HTTPError("", 412, "CAS conflict", {}, None)
        new_generation = str(int(previous) + 1)
        self.objects[key] = (deepcopy(value), new_generation)
        self.writes.append(key)
        return new_generation

    def call(self, method, url):
        if method != "GET":
            raise AssertionError("inventory is read-only")
        prefix = parse_qs(urlparse(url).query)["prefix"][0]
        return json.dumps(
            {
                "items": [
                    {"name": key}
                    for key in sorted(self.objects)
                    if key.startswith(prefix)
                ]
            }
        ).encode()


class ReceiptBoundaryTests(unittest.TestCase):
    @patch("notifications.time.time", return_value=NOW)
    def test_fixed_routing_and_plain_text_use_only_confirmed_receipt_fields(self, _):
        cases = [
            (
                "develop",
                CONFIG,
                "stg.deployment@aidash.run",
                "e" * 32 + ".preview.aidash.run",
            ),
            (
                "pr-74",
                CONFIG,
                "test.deployment@aidash.run",
                "e" * 32 + ".preview.aidash.run",
            ),
            ("test", CONFIG, "test.deployment@aidash.run", "test.aidash.run"),
            (
                "production",
                {
                    "deployment_tier": "production",
                    "production_origin": "https://aidash.run",
                },
                "prod.deployment@aidash.run",
                "aidash.run",
            ),
        ]
        for identity, config, recipient, hostname in cases:
            with self.subTest(identity=identity):
                payload = message(receipt(identity, hostname=hostname), config)
                self.assertEqual(payload["from"], "deployment.noreply@aidash.run")
                self.assertEqual(payload["to"], [recipient])
                self.assertIn("URL: https://" + hostname, payload["text"])
                self.assertIn(
                    "https://github.com/kent8192/aidash/actions/runs/1234",
                    payload["text"],
                )
                self.assertNotIn("html", payload)
        failed = message(receipt(status="failed"), CONFIG)
        self.assertIn("deployment failed", failed["subject"])
        self.assertNotIn("URL:", failed["text"])

    @patch("notifications.time.time", return_value=NOW)
    def test_untrusted_routing_headers_hosts_revisions_or_scope_never_reach_transport(
        self, _
    ):
        cases = [
            {"to": "attacker@example.com"},
            {"headers": {"Bcc": "attacker@example.com"}},
            {"hostname": "e" * 32 + ".preview.aidash.run.attacker.example"},
            {"hostname": "e" * 32 + ".preview.aidash.run\r\nBcc: attacker@example.com"},
            {"hostname": "attacker.example@" + "e" * 32 + ".preview.aidash.run"},
            {"hostname": "e" * 32 + ".preview.aidash.run/path"},
            {"hostname": "e" * 32 + ".preview.aidash.run:443"},
            {"protocol": "other/1"},
            {"source_sha": "a" * 39 + "\n"},
            {"run_id": "1234?token=secret"},
            {"generation": True},
            {"status": "pending"},
            {"occurred_at": NOW + 301},
            {"images": {"app": "image:latest"}},
            {"environment": "production", "hostname": "aidash.run"},
        ]
        for changes in cases:
            with self.subTest(changes=changes):
                store, sender = MemoryStore([receipt(**changes)]), Mock()
                with self.assertRaises(Refused) as failure:
                    drain(store, CONFIG, sender)
                self.assertNotIn("attacker", str(failure.exception))
                sender.send.assert_not_called()
                self.assertFalse(store.writes)
        with self.assertRaises(Refused):
            message(
                receipt("production", hostname="aidash.run"),
                {"deployment_tier": "production"},
            )

    @patch("notifications.time.time", return_value=NOW)
    def test_pre_update_backup_fields_are_optional_but_exact(self, _):
        name = "projects/aidash-test/locations/us-central1/backupPlans/aidash-production-pre-update/backups/pre-update-g4-" + "a" * 12
        production = {"deployment_tier": "production"}
        plain = receipt("production", hostname="aidash.run")
        configuration = dict(production, production_origin="https://aidash.run")
        self.assertNotIn("Pre-update Backup", message(plain, configuration)["text"])
        text = message(dict(plain, pre_update_backup=name, writers_started=True), configuration)["text"]
        self.assertIn(f"Pre-update Backup: {name}", text)
        self.assertIn("Writers started: yes", text)
        self.assertNotIn("Pre-update Backup", message(dict(plain, writers_started=True), configuration)["text"])
        for changes in (
            {"pre_update_backup": "projects/x/backups/y"},
            {"pre_update_backup": name + "\nBcc: attacker@example.com"},
            {"pre_update_backup": name, "writers_started": "yes"},
            {"writers_started": False},
            {"unexpected": True},
        ):
            with self.subTest(changes=changes), self.assertRaises(Refused):
                message(dict(plain, **changes), configuration)

    @patch("notifications.time.time", return_value=NOW)
    def test_receipt_object_name_must_match_event_identity(self, _):
        store = MemoryStore([receipt()])
        old = store.objects.pop("notifications/" + "a" * 64 + ".json")
        store.objects["notifications/" + "b" * 64 + ".json"] = old
        sender = Mock()
        with self.assertRaises(Refused):
            drain(store, CONFIG, sender)
        sender.send.assert_not_called()


class CloudflareTransportTests(unittest.TestCase):
    def sender(self):
        return CloudflareSender("1" * 32, "protected_api_token_value")

    def test_transport_refuses_routing_or_header_override_even_without_receipt_builder(
        self,
    ):
        changes = [
            {"to": "attacker@example.com"},
            # Cloudflare's API takes an array; a bare address is not the fixed shape.
            {"to": "stg.deployment@aidash.run"},
            {"to": ["attacker@example.com"]},
            {"to": ["stg.deployment@aidash.run", "attacker@example.com"]},
            {"to": []},
            {"from": "attacker@example.com"},
            {"bcc": "attacker@example.com"},
            {"subject": "status\r\nBcc: attacker@example.com"},
            {"headers": {"Bcc": "attacker@example.com"}},
        ]
        for change in changes:
            with self.subTest(change=change):
                sender, payload = self.sender(), mail_payload()
                sender.opener.open = Mock()
                payload.update(change)
                with self.assertRaises(Refused):
                    sender.send(payload)
                sender.opener.open.assert_not_called()

    def test_https_verified_fixed_api_request_and_provider_delivery_result(self):
        for result, expected in (
            ({"queued": ["stg.deployment@aidash.run"]}, "accepted"),
            ({"delivered": ["stg.deployment@aidash.run"]}, "delivered"),
        ):
            with self.subTest(result=result):
                sender = self.sender()
                sender.opener.open = Mock(
                    return_value=io.BytesIO(
                        json.dumps({"success": True, "result": result}).encode()
                    )
                )
                with patch("notifications.time.time", return_value=NOW):
                    self.assertEqual(sender.send(message(receipt(), CONFIG)), expected)
                request = sender.opener.open.call_args.args[0]
                self.assertEqual(
                    request.full_url,
                    "https://api.cloudflare.com/client/v4/accounts/"
                    + "1" * 32
                    + "/email/sending/send",
                )
                self.assertEqual(request.get_method(), "POST")
                self.assertEqual(sender.opener.open.call_args.kwargs["timeout"], 30)
                payload = json.loads(request.data)
                self.assertEqual(payload["to"], ["stg.deployment@aidash.run"])
                self.assertEqual(payload["headers"]["X-Aidash-Event-ID"], "a" * 64)
                self.assertNotIn("Message-ID", payload["headers"])
                handler = next(
                    item
                    for item in sender.opener.handlers
                    if isinstance(item, urllib.request.HTTPSHandler)
                )
                self.assertEqual(handler._context.verify_mode, ssl.CERT_REQUIRED)
                self.assertTrue(handler._context.check_hostname)

    def test_redirects_cannot_forward_the_api_token(self):
        sender = self.sender()
        redirect = next(
            item for item in sender.opener.handlers if hasattr(item, "redirect_request")
        )
        request = urllib.request.Request(
            sender.endpoint,
            headers={"Authorization": "Bearer protected_api_token_value"},
        )
        self.assertIsNone(
            redirect.redirect_request(
                request, None, 302, "redirect", {}, "https://attacker.example"
            )
        )

    def test_api_failures_are_redacted_and_retryable_only_for_transport_quota_or_server_errors(
        self,
    ):
        failures = [
            (
                HTTPError(
                    "private-url", 429, "private-error", {"Retry-After": "1800"}, None
                ),
                True,
                1800,
            ),
            (HTTPError("private-url", 503, "private-error", {}, None), True, 300),
            (
                HTTPError("private-url", 401, "protected_api_token_value", {}, None),
                False,
                300,
            ),
            (HTTPError("private-url", 302, "private-error", {}, None), False, 300),
            (URLError("protected_api_token_value"), True, 300),
            (TimeoutError("private-url"), True, 300),
        ]
        for error, retryable, delay in failures:
            with self.subTest(error=type(error), status=getattr(error, "code", None)):
                sender = self.sender()
                sender.opener.open = Mock(side_effect=error)
                with self.assertRaises(SendFailure) as failure:
                    sender.send(mail_payload())
                self.assertEqual(failure.exception.retryable, retryable)
                self.assertEqual(failure.exception.delay, delay)
                self.assertNotIn("protected_api_token_value", str(failure.exception))
                self.assertNotIn("private-url", str(failure.exception))
                self.assertEqual(sender.opener.open.call_count, 1)

    def test_bounces_suppression_and_accepted_for_another_recipient_are_not_success(
        self,
    ):
        results = [
            ({"permanent_bounces": ["stg.deployment@aidash.run"]}, False),
            ({"suppressed_recipients": ["stg.deployment@aidash.run"]}, False),
            ({"queued": ["attacker@example.com"]}, True),
            ({}, True),
        ]
        for result, retryable in results:
            with self.subTest(result=result):
                sender = self.sender()
                sender.opener.open = Mock(
                    return_value=io.BytesIO(
                        json.dumps({"success": True, "result": result}).encode()
                    )
                )
                with self.assertRaises(SendFailure) as failure:
                    sender.send(mail_payload())
                self.assertEqual(failure.exception.retryable, retryable)


class DurableDeliveryTests(unittest.TestCase):
    @patch("notifications.time.time", return_value=NOW)
    def test_concurrent_notifiers_and_repeated_runs_send_once_after_acknowledgement(
        self, _
    ):
        store, sender = MemoryStore([receipt()]), Mock()

        def concurrent_send(payload):
            other = Mock()
            self.assertEqual(drain(store, CONFIG, other)["skipped"], 1)
            other.send.assert_not_called()
            return "accepted"

        sender.send.side_effect = concurrent_send
        self.assertEqual(drain(store, CONFIG, sender)["accepted"], 1)
        self.assertEqual(drain(store, CONFIG, sender)["skipped"], 1)
        self.assertEqual(sender.send.call_count, 1)
        self.assertTrue(
            all(key.startswith("notification-delivery/") for key in store.writes)
        )
        self.assertNotIn("lifecycle/state.json", store.objects)

    def test_ambiguous_acceptance_retries_only_after_backoff_without_redeployment(self):
        store, sender = MemoryStore([receipt()]), Mock()
        sender.send.side_effect = [SendFailure(True, 900), "delivered"]
        with patch("notifications.time.time", return_value=NOW):
            self.assertEqual(drain(store, CONFIG, sender)["retry"], 1)
            self.assertEqual(drain(store, CONFIG, sender)["skipped"], 1)
        with patch("notifications.time.time", return_value=NOW + 899):
            self.assertEqual(drain(store, CONFIG, sender)["skipped"], 1)
        with patch("notifications.time.time", return_value=NOW + 900):
            self.assertEqual(drain(store, CONFIG, sender)["delivered"], 1)
        self.assertEqual(sender.send.call_count, 2)
        event_ids = [
            call.args[0]["headers"]["X-Aidash-Event-ID"]
            for call in sender.send.call_args_list
        ]
        self.assertEqual(event_ids, ["a" * 64, "a" * 64])
        self.assertEqual(
            store.read("notification-delivery/" + "a" * 64 + ".json")[0]["status"],
            "delivered",
        )

    @patch("notifications.time.time", return_value=NOW)
    def test_permanent_rejection_requires_explicit_notifier_retry(self, _):
        store, sender = MemoryStore([receipt()]), Mock()
        sender.send.side_effect = [SendFailure(False), "accepted"]
        self.assertEqual(drain(store, CONFIG, sender)["blocked"], 1)
        self.assertEqual(drain(store, CONFIG, sender)["skipped"], 1)
        self.assertEqual(sender.send.call_count, 1)
        self.assertEqual(
            drain(store, CONFIG, sender, retry_blocked=True)["accepted"], 1
        )
        self.assertEqual(sender.send.call_count, 2)

    @patch("notifications.time.time", return_value=NOW)
    def test_lost_acknowledgement_does_not_immediately_resend(self, _):
        store, sender = MemoryStore([receipt()]), Mock()
        sender.send.return_value = "accepted"
        write = store.write

        def lose_ack(key, value, generation):
            if value["status"] == "accepted":
                raise OSError("lost acknowledgement")
            return write(key, value, generation)

        with (
            patch.object(store, "write", side_effect=lose_ack),
            self.assertRaises(OSError),
        ):
            drain(store, CONFIG, sender)
        self.assertEqual(drain(store, CONFIG, sender)["skipped"], 1)
        self.assertEqual(sender.send.call_count, 1)


class TriggerAuthorizationTests(unittest.TestCase):
    def test_known_completed_deployment_only_not_ci_fork_or_other_workflow(self):
        event = {
            "repository": {"full_name": "kent8192/aidash"},
            "workflow_run": {
                "workflow_id": 42,
                "status": "completed",
                "head_repository": {"full_name": "kent8192/aidash"},
            },
        }
        for path, name, allowed in (
            (".github/workflows/gcp-environments.yml", "GCP environments", True),
            (".github/workflows/gcp-production.yml", "GCP production", True),
            (".github/workflows/ci.yml", "CI", False),
            (".github/workflows/release-ci.yml", "Release CI", False),
            (".github/workflows/other.yml", "GCP environments", False),
        ):
            with (
                self.subTest(path=path),
                patch.dict(os.environ, {"GITHUB_REPOSITORY": "kent8192/aidash"}),
                patch(
                    "notifications.github", return_value={"path": path, "name": name}
                ),
            ):
                if allowed:
                    preflight("workflow_run", event)
                else:
                    with self.assertRaises(Refused):
                        preflight("workflow_run", event)
        for change in (
            {"status": "in_progress"},
            {"head_repository": {"full_name": "attacker/fork"}},
        ):
            forbidden = deepcopy(event)
            forbidden["workflow_run"].update(change)
            with (
                patch.dict(os.environ, {"GITHUB_REPOSITORY": "kent8192/aidash"}),
                patch("notifications.github") as github,
            ):
                with self.assertRaises(Refused):
                    preflight("workflow_run", forbidden)
                github.assert_not_called()

    def test_manual_retry_requires_current_write_permission(self):
        event = {"repository": {"full_name": "kent8192/aidash"}}
        with patch.dict(
            os.environ,
            {"GITHUB_REPOSITORY": "kent8192/aidash", "GITHUB_ACTOR": "maintainer"},
        ):
            for permission in ("read", "triage", "none"):
                with (
                    patch(
                        "notifications.github", return_value={"permission": permission}
                    ),
                    self.assertRaises(Refused),
                ):
                    preflight("workflow_dispatch", event)
            with patch("notifications.github", return_value={"permission": "write"}):
                preflight("workflow_dispatch", event)


if __name__ == "__main__":
    unittest.main()
