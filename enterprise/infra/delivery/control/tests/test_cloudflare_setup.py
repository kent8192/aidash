# SPDX-FileCopyrightText: 2026 kent8192
# SPDX-License-Identifier: BUSL-1.1
# See enterprise/LICENSE.

"""Adoption, DNS conflict and one-time credential handoff contracts."""

import io
import json
import os
import ssl
import subprocess
import sys
import unittest
import urllib.request
from copy import deepcopy
from pathlib import Path
from tempfile import TemporaryDirectory
from unittest.mock import Mock, patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from setup import Refused
from cloudflare_setup import (
    ALIASES,
    CloudflareAPI,
    SetupError,
    bootstrap_token,
    check_routing_dns,
    discover_existing,
    gh,
    provision_sending_token,
)

ACCOUNT, ZONE, TOKEN_ID = "a" * 32, "b" * 32, "c" * 32
SECRET_VALUE = "cfat_" + "s" * 46


class AdoptionTests(unittest.TestCase):
    def fixture(self):
        api = Mock()
        api.call.side_effect = [
            {
                "result": {
                    "name": "aidash.run",
                    "status": "active",
                    "account": {"id": ACCOUNT},
                }
            },
            {"result": {"name": "aidash.run", "enabled": True, "status": "ready"}},
        ]
        api.inventory.side_effect = [
            [{"name": "aidash.run", "tag": "d" * 32, "enabled": True}],
            [
                {
                    "email": "private@example.net",
                    "id": "e" * 32,
                    "verified": "2026-10-12T00:00:00Z",
                }
            ],
            [],
        ]
        return api

    def test_existing_verified_domain_destination_and_rules_are_only_read_and_adopted(
        self,
    ):
        api = self.fixture()
        values = discover_existing(api, ACCOUNT, ZONE, "private@example.net")
        self.assertEqual(values["sending_subdomain_id"], "d" * 32)
        self.assertEqual(values["destination_address_id"], "e" * 32)
        self.assertEqual(values["destination_email"], "private@example.net")
        self.assertTrue(all(call.args[0] == "GET" for call in api.call.call_args_list))

    def test_unverified_or_duplicate_destination_and_alias_worker_ownership_are_refused(
        self,
    ):
        scenarios = [
            [{"email": "private@example.net", "id": "e" * 32, "verified": None}],
            [{"email": "private@example.net", "id": "e" * 32, "verified": "yes"}] * 2,
        ]
        for destination in scenarios:
            api = self.fixture()
            api.inventory.side_effect = [
                [{"name": "aidash.run", "tag": "d" * 32, "enabled": True}],
                destination,
                [],
            ]
            with self.assertRaises(SetupError):
                discover_existing(api, ACCOUNT, ZONE, "private@example.net")
        for rule_changes in (
            {"source": "wrangler"},
            {"actions": [{"type": "worker", "value": ["private-worker"]}]},
            {"duplicate": True},
        ):
            api = self.fixture()
            rule = {
                "id": "f" * 32,
                "source": "api",
                "matchers": [
                    {"type": "literal", "field": "to", "value": ALIASES["stg"]}
                ],
                "actions": [{"type": "forward", "value": ["private@example.net"]}],
            }
            rule.update(
                {
                    key: value
                    for key, value in rule_changes.items()
                    if key != "duplicate"
                }
            )
            api.inventory.side_effect = [
                [{"name": "aidash.run", "tag": "d" * 32, "enabled": True}],
                [{"email": "private@example.net", "id": "e" * 32, "verified": "yes"}],
                [rule, deepcopy(rule)] if rule_changes.get("duplicate") else [rule],
            ]
            with self.assertRaises(SetupError):
                discover_existing(api, ACCOUNT, ZONE, "private@example.net")

    def test_malformed_zone_id_is_never_guessed_or_sent_to_api(self):
        api = Mock()
        with self.assertRaises(SetupError):
            discover_existing(api, ACCOUNT, "malformed-zone-id", "private@example.net")
        api.call.assert_not_called()

    def test_opt_in_dns_does_not_replace_external_mx_or_duplicate_spf(self):
        proposal = [
            {"type": "MX", "name": "@", "content": "route1.mx.cloudflare.net"},
            {
                "type": "TXT",
                "name": "aidash.run",
                "content": "v=spf1 include:_spf.mx.cloudflare.net ~all",
            },
        ]
        safe = [
            {
                "type": "MX",
                "name": "aidash.run",
                "content": "route1.mx.cloudflare.net.",
            },
            {
                "type": "TXT",
                "name": "aidash.run",
                "content": '"v=spf1 include:_spf.mx.cloudflare.net ~all"',
            },
        ]
        check_routing_dns([], proposal)
        check_routing_dns(safe, proposal)
        conflicts = [
            [{"type": "MX", "name": "aidash.run", "content": "aspmx.l.google.com"}],
            [
                {
                    "type": "TXT",
                    "name": "aidash.run",
                    "content": "v=spf1 include:_spf.google.com ~all",
                }
            ],
            safe + [safe[1]],
        ]
        for records in conflicts:
            with self.subTest(records=records), self.assertRaises(SetupError):
                check_routing_dns(records, proposal)


class TokenHandoffTests(unittest.TestCase):
    def setUp(self):
        guard = patch("cloudflare_setup.verify_environment", return_value={"protected": True})
        self.protection = guard.start()
        self.addCleanup(guard.stop)

    def api(self):
        api = Mock()
        api.inventory.return_value = [
            {
                "id": "f" * 32,
                "name": "Email Sending Write",
                "scopes": ["com.cloudflare.api.account"],
                "is_selectable": True,
            }
        ]
        api.call.side_effect = [
            {"success": True, "result": []},
            {"success": True, "result": {"id": TOKEN_ID, "value": SECRET_VALUE}},
            {"success": True, "result": {"id": TOKEN_ID}},
        ]
        return api

    def test_existing_secret_or_plan_never_creates_a_token(self):
        api = self.api()
        with patch(
            "cloudflare_setup.gh",
            return_value=b'[{"name":"CLOUDFLARE_EMAIL_SENDING_TOKEN"}]',
        ):
            self.assertEqual(
                provision_sending_token(api, ACCOUNT, apply=True)["status"],
                "existing_secret",
            )
            api.call.assert_not_called()
        with patch("cloudflare_setup.gh", return_value=b"[]"):
            self.assertEqual(provision_sending_token(api, ACCOUNT)["status"], "planned")
            self.assertTrue(
                all(call.args[0] == "GET" for call in api.call.call_args_list)
            )

    def test_removed_protection_blocks_creation_and_revokes_after_a_handoff_race(self):
        api = self.api()
        self.protection.side_effect = Refused("missing protection")
        with patch("cloudflare_setup.gh", return_value=b"[]") as handoff, self.assertRaises(Refused):
            provision_sending_token(api, ACCOUNT, apply=True)
        self.assertTrue(all(call.args[0] == "GET" for call in api.call.call_args_list))
        self.assertEqual(handoff.call_count, 1)
        api = self.api()
        self.protection.side_effect = [{"protected": True}, Refused("protection changed")]
        with patch("cloudflare_setup.gh", return_value=b"[]") as handoff, self.assertRaises(SetupError):
            provision_sending_token(api, ACCOUNT, apply=True)
        self.assertEqual(api.call.call_args_list[-1].args, ("DELETE", f"accounts/{ACCOUNT}/tokens/{TOKEN_ID}"))
        self.assertEqual(handoff.call_count, 1)

    def test_create_is_account_scoped_and_value_goes_only_to_secret_stdin(self):
        api, receipts = self.api(), []
        with patch("cloudflare_setup.gh", side_effect=[b"[]", b""]) as github:
            outcome = provision_sending_token(
                api, ACCOUNT, apply=True, journal=receipts.append
            )
        policy = api.call.call_args_list[1].args[2]
        self.assertEqual(
            policy["policies"],
            [
                {
                    "effect": "allow",
                    "permission_groups": [{"id": "f" * 32}],
                    "resources": {f"com.cloudflare.api.account.{ACCOUNT}": "*"},
                }
            ],
        )
        self.assertEqual(github.call_args_list[1].kwargs["data"], SECRET_VALUE.encode())
        self.assertNotIn(SECRET_VALUE, " ".join(github.call_args_list[1].args))
        self.assertEqual(outcome["status"], "secret_installed")
        self.assertNotIn(SECRET_VALUE, json.dumps(receipts) + json.dumps(outcome))

    def test_failed_handoff_revokes_new_token_and_records_only_cleanup_metadata(self):
        api, receipts = self.api(), []
        with (
            patch(
                "cloudflare_setup.gh",
                side_effect=[b"[]", SetupError("private operation failed")],
            ),
            self.assertRaises(SetupError) as failure,
        ):
            provision_sending_token(api, ACCOUNT, apply=True, journal=receipts.append)
        self.assertEqual(
            api.call.call_args_list[-1].args,
            ("DELETE", f"accounts/{ACCOUNT}/tokens/{TOKEN_ID}"),
        )
        self.assertEqual(
            failure.exception.receipt["status"], "revoked_after_handoff_failure"
        )
        self.assertNotIn(SECRET_VALUE, json.dumps(receipts) + str(failure.exception))

    def test_active_orphan_token_is_not_duplicated(self):
        api = self.api()
        api.call.side_effect = [
            {
                "result": [
                    {
                        "id": TOKEN_ID,
                        "name": "aidash-deployment-notifications",
                        "status": "active",
                    }
                ]
            }
        ]
        with (
            patch("cloudflare_setup.gh", return_value=b"[]"),
            self.assertRaises(SetupError),
        ):
            provision_sending_token(api, ACCOUNT, apply=True)
        self.assertTrue(all(call.args[0] == "GET" for call in api.call.call_args_list))

    def test_broad_or_unselectable_permission_never_creates_a_token(self):
        for changes in (
            {"name": "DNS Write"},
            {"scopes": ["com.cloudflare.api.account.zone"]},
            {"is_selectable": False},
        ):
            api = self.api()
            group = api.inventory.return_value[0]
            group.update(changes)
            with (
                patch("cloudflare_setup.gh", return_value=b"[]"),
                self.assertRaises(SetupError),
            ):
                provision_sending_token(api, ACCOUNT, apply=True)
            api.call.assert_not_called()


class InventoryPaginationTests(unittest.TestCase):
    PATH = f"accounts/{ACCOUNT}/email/routing/addresses"

    @staticmethod
    def page(items, total, per_page=50):
        return {"success": True, "result": items, "result_info": {"total_count": total, "per_page": per_page, "count": len(items)}}

    def api(self, *pages):
        api = CloudflareAPI("bootstrap_token_private_value")
        api.call = Mock(side_effect=list(pages))
        return api

    def test_every_page_is_read_before_adoption(self):
        first = [{"id": f"{index:032x}"} for index in range(50)]
        second = [{"id": f"{index:032x}"} for index in range(50, 60)]
        api = self.api(self.page(first, 60), self.page(second, 60))
        self.assertEqual(api.inventory(self.PATH), first + second)
        self.assertEqual(
            [call.args for call in api.call.call_args_list],
            [("GET", self.PATH), ("GET", self.PATH + "?page=2&per_page=50")],
        )

    def test_existing_query_is_kept_and_its_page_size_replaced(self):
        path = f"zones/{ZONE}/dns_records?name=aidash.run&per_page=100"
        api = self.api(self.page([{"id": "1"}], 2, 1), self.page([{"id": "2"}], 2, 1))
        self.assertEqual(len(api.inventory(path)), 2)
        self.assertEqual(api.call.call_args_list[1].args[1], f"zones/{ZONE}/dns_records?name=aidash.run&per_page=1&page=2")

    def test_single_page_without_result_info_is_complete(self):
        api = self.api({"success": True, "result": [{"id": "1"}]})
        self.assertEqual(api.inventory(self.PATH), [{"id": "1"}])
        api.call.assert_called_once()

    def test_repeated_empty_oversized_or_unpageable_inventories_are_refused(self):
        item = {"id": "1"}
        for pages in (
            # A page that repeats an earlier one did not advance.
            (self.page([item], 2, 1), self.page([item], 2, 1)),
            # An empty page before total_count is reached.
            (self.page([item], 2, 1), self.page([], 2, 1)),
            # More items than the reported total.
            (self.page([item, {"id": "2"}], 1),),
            # Incomplete with no usable page size to continue.
            (self.page([item], 3, 0),),
        ):
            with self.subTest(pages=pages), self.assertRaises(SetupError):
                self.api(*pages).inventory(self.PATH)
        endless = [self.page([{"id": str(index)}], 1000, 1) for index in range(3)]
        with self.assertRaisesRegex(SetupError, "page limit"):
            self.api(*endless).inventory(self.PATH, max_pages=3)


class SecretTransportTests(unittest.TestCase):
    def test_provider_tls_certificate_verification_and_redirect_rejection(self):
        api = CloudflareAPI("bootstrap_token_private_value")
        api.opener.open = Mock(return_value=io.BytesIO(b'{"success":true,"result":[]}'))
        api.call("GET", f"accounts/{ACCOUNT}/tokens")
        self.assertEqual(
            api.opener.open.call_args.args[0].full_url,
            f"https://api.cloudflare.com/client/v4/accounts/{ACCOUNT}/tokens",
        )
        tls = next(
            item
            for item in api.opener.handlers
            if isinstance(
                item,
                urllib.request.HTTPSHandler,
            )
        )
        self.assertEqual(tls._context.verify_mode, ssl.CERT_REQUIRED)
        self.assertTrue(tls._context.check_hostname)
        redirect = next(
            item for item in api.opener.handlers if hasattr(item, "redirect_request")
        )
        self.assertIsNone(
            redirect.redirect_request(
                None, None, 302, "redirect", {}, "https://attacker.example"
            )
        )

    def test_gh_receives_generated_value_only_on_stdin_and_no_bootstrap_environment(
        self,
    ):
        with (
            patch.dict(
                os.environ, {"CLOUDFLARE_API_TOKEN": "bootstrap_token_private_value"}
            ),
            patch(
                "cloudflare_setup.subprocess.run",
                return_value=subprocess.CompletedProcess([], 0, b"", b""),
            ) as command,
        ):
            gh(
                "secret",
                "set",
                "CLOUDFLARE_EMAIL_SENDING_TOKEN",
                data=SECRET_VALUE.encode(),
            )
        self.assertEqual(command.call_args.kwargs["input"], SECRET_VALUE.encode())
        self.assertNotIn("CLOUDFLARE_API_TOKEN", command.call_args.kwargs["env"])
        self.assertNotIn(SECRET_VALUE, command.call_args.args[0])

    def test_bootstrap_file_requires_private_permissions(self):
        with TemporaryDirectory() as temporary:
            source = Path(temporary) / "bootstrap-token"
            source.write_text("bootstrap_token_private_value")
            source.chmod(0o644)
            with self.assertRaises(SetupError):
                bootstrap_token(source)
            source.chmod(0o600)
            self.assertEqual(bootstrap_token(source), "bootstrap_token_private_value")


if __name__ == "__main__":
    unittest.main()
