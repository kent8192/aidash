"""Tenant IAM uses fake REST policies, including concurrent operator changes."""

from copy import deepcopy
import json
from pathlib import Path
import sys
import unittest
from unittest.mock import patch
from urllib.error import HTTPError

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "control"))
from gcip import TenantIAM, TenantMFA, READ_ROLE, reconcile_environment, reconcile_mfa
import controller

MEMBER = "serviceAccount:runtime@aidash-fixture.iam.gserviceaccount.com"
OUTPUT = {
    "runtime_secret": "runtime",
    "gcip": {
        "project_id": "aidash-fixture",
        "public_origin": "https://test.aidash.run",
        "tenant_ids": ["pool-a"],
        "runtime_service_account": MEMBER.split(":", 1)[1],
        "tenant_bindings": {"pool-a": "acme"},
        "providers": {"pool-a": ["password"]},
        "password_sign_up": ["pool-a"],
    },
}


class FakeAPI(TenantIAM):
    def __init__(self):
        self.policy = {
            "version": 3,
            "etag": "first",
            "bindings": [
                {
                    "role": "roles/identityplatform.admin",
                    "members": ["user:operator@example.test"],
                },
                {
                    "role": READ_ROLE,
                    "members": ["user:auditor@example.test"],
                    "condition": {"title": "temporary", "expression": "true"},
                },
            ],
            "auditConfigs": [{"service": "allServices"}],
        }
        self.sets = []
        self.conflict = False

    def call(self, resource, method, body):
        assert resource == "projects/aidash-fixture/tenants/pool-a"
        if method == "getIamPolicy":
            assert body == {"options": {"requestedPolicyVersion": 3}}
            return deepcopy(self.policy)
        assert method == "setIamPolicy" and body["updateMask"] == "bindings,etag"
        assert body["policy"]["etag"] == self.policy["etag"]
        if self.conflict:
            self.conflict = False
            self.policy["etag"] = "concurrent"
            self.policy["bindings"].append(
                {"role": READ_ROLE, "members": ["user:new@example.test"]}
            )
            raise HTTPError("", 409, "conflict", {}, None)
        self.sets.append(deepcopy(body))
        self.policy = deepcopy(body["policy"])
        return deepcopy(self.policy)


class TenantIAMTests(unittest.TestCase):
    def test_add_idempotence_and_unrelated_conditional_policy_preservation(self):
        api = FakeAPI()
        original = deepcopy(api.policy)
        reconcile_environment(OUTPUT, api=api)
        reconcile_environment(OUTPUT, api=api)
        self.assertEqual(len(api.sets), 1)
        self.assertEqual(api.policy["bindings"][:2], original["bindings"])
        self.assertEqual(api.policy["auditConfigs"], original["auditConfigs"])
        self.assertEqual(
            api.policy["bindings"][2], {"role": READ_ROLE, "members": [MEMBER]}
        )

    def test_etag_conflict_refetches_and_preserves_the_concurrent_binding(self):
        api = FakeAPI()
        api.conflict = True
        reconcile_environment(OUTPUT, api=api)
        self.assertEqual(api.policy["etag"], "concurrent")
        self.assertIn(MEMBER, api.policy["bindings"][2]["members"])
        self.assertIn("user:new@example.test", api.policy["bindings"][2]["members"])

    def test_destroy_removes_only_our_managed_member(self):
        api = FakeAPI()
        reconcile_environment(OUTPUT, api=api)
        api.policy["bindings"][-1]["members"].append("user:unrelated@example.test")
        reconcile_environment(OUTPUT, enabled=False, api=api)
        self.assertEqual(
            api.policy["bindings"][-1]["members"], ["user:unrelated@example.test"]
        )
        reconcile_environment(OUTPUT, enabled=False, api=api)
        self.assertEqual(len(api.sets), 2)

    def test_refuses_existing_policy_without_etag(self):
        api = FakeAPI()
        del api.policy["etag"]
        with self.assertRaisesRegex(RuntimeError, "etag"):
            reconcile_environment(OUTPUT, api=api)
        self.assertFalse(api.sets)

    def test_missing_destroyed_tenant_is_idempotent(self):
        api = FakeAPI()
        with patch.object(
            api, "call", side_effect=HTTPError("", 404, "gone", {}, None)
        ):
            reconcile_environment(OUTPUT, enabled=False, api=api)
            with self.assertRaises(HTTPError):
                reconcile_environment(OUTPUT, api=api)

    def test_rest_transport_uses_exact_tenant_resource(self):
        class Response:
            def __enter__(self):
                return self

            def __exit__(self, *args):
                pass

            def read(self):
                return b'{"etag":"fixture"}'

        with (
            patch("gcip.run", return_value=b"token"),
            patch("gcip.urllib.request.urlopen", return_value=Response()) as request,
        ):
            TenantIAM().call(
                "projects/aidash-fixture/tenants/pool-a", "getIamPolicy", {}
            )
        sent = request.call_args.args[0]
        self.assertEqual(
            sent.full_url,
            "https://identitytoolkit.googleapis.com/admin/v2/projects/aidash-fixture/tenants/pool-a:getIamPolicy",
        )
        self.assertEqual(sent.get_header("Authorization"), "Bearer token")


class FakeMFA(TenantMFA):
    def __init__(self, tenant):
        self.tenant = tenant
        self.patches = []

    def call(self, method, resource, body=None):
        if method == "GET":
            assert resource == "projects/aidash-fixture/tenants/pool-a" and body is None
            return deepcopy(self.tenant)
        assert method == "PATCH"
        assert resource == "projects/aidash-fixture/tenants/pool-a?updateMask=mfaConfig"
        self.patches.append(deepcopy(body))
        self.tenant["mfaConfig"] = deepcopy(body["mfaConfig"])
        return deepcopy(self.tenant)


DISABLED = {"state": "DISABLED", "enabledProviders": [], "providerConfigs": []}
DECLARED = dict(OUTPUT, gcip=dict(OUTPUT["gcip"], mfa={"pool-a": "disabled"}))


class TenantMFATests(unittest.TestCase):
    def test_new_tenant_without_mfa_config_already_holds_the_disabled_requirement(self):
        api = FakeMFA({"name": "projects/aidash-fixture/tenants/pool-a"})
        self.assertEqual(reconcile_mfa(DECLARED, api=api), {})
        self.assertFalse(api.patches)

    def test_enabled_factors_are_drift_and_replaced_by_the_complete_declared_config(self):
        for actual, fields in (
            ({"state": "ENABLED", "enabledProviders": ["PHONE_SMS"]}, ["enabledProviders", "state"]),
            ({"state": "DISABLED", "providerConfigs": [{"state": "ENABLED", "totpProviderConfig": {"adjacentIntervals": 5}}]}, ["providerConfigs"]),
            ({"state": "MANDATORY"}, ["state"]),
        ):
            with self.subTest(actual=actual):
                api = FakeMFA({"mfaConfig": actual})
                self.assertEqual(reconcile_mfa(DECLARED, api=api), {"pool-a": fields})
                self.assertEqual(api.patches, [{"mfaConfig": DISABLED}])
                self.assertEqual(reconcile_mfa(DECLARED, api=api), {})
                self.assertEqual(len(api.patches), 1)

    def test_inert_provider_configs_are_not_drift(self):
        api = FakeMFA({"mfaConfig": {"state": "STATE_UNSPECIFIED", "enabledProviders": ["PROVIDER_UNSPECIFIED"], "providerConfigs": [{"state": "DISABLED", "totpProviderConfig": {"adjacentIntervals": 5}}]}})
        self.assertEqual(reconcile_mfa(DECLARED, api=api), {})
        self.assertFalse(api.patches)

    def test_state_without_a_declaration_is_left_for_its_next_apply(self):
        api = FakeMFA({"mfaConfig": {"state": "ENABLED"}})
        self.assertEqual(reconcile_mfa(OUTPUT, api=api), {})
        self.assertFalse(api.patches)

    def test_unsupported_requirement_is_refused_before_any_write(self):
        api = FakeMFA({"mfaConfig": {"state": "ENABLED"}})
        output = dict(OUTPUT, gcip=dict(OUTPUT["gcip"], mfa={"pool-a": "mandatory"}))
        with self.assertRaisesRegex(ValueError, "Unsupported MFA Requirement"):
            reconcile_mfa(output, api=api)
        self.assertFalse(api.patches)

    def test_rest_transport_reads_and_patches_only_the_tenant_mfa_config(self):
        class Response:
            def __enter__(self):
                return self

            def __exit__(self, *args):
                pass

            def read(self):
                return b'{"mfaConfig":{"state":"ENABLED"}}'

        with (
            patch("gcip.run", return_value=b"token"),
            patch("gcip.urllib.request.urlopen", return_value=Response()) as request,
        ):
            TenantMFA().reconcile("aidash-fixture", "pool-a", "disabled")
        read, write = (call.args[0] for call in request.call_args_list)
        base = "https://identitytoolkit.googleapis.com/v2/projects/aidash-fixture/tenants/pool-a"
        self.assertEqual((read.get_method(), read.full_url, read.data), ("GET", base, None))
        self.assertEqual((write.get_method(), write.full_url), ("PATCH", base + "?updateMask=mfaConfig"))
        self.assertEqual(json.loads(write.data), {"mfaConfig": DISABLED})
        self.assertEqual(write.get_header("Authorization"), "Bearer token")


class RuntimeConfigTests(unittest.TestCase):
    def test_outputs_replace_runtime_bindings_without_republishing_unchanged_secrets(
        self,
    ):
        raw = {
            "node": {"api_token": "private"},
            "dashboard": {"gcip": {"web_api_key": "public"}},
        }
        calls = []

        def command(*args, **kwargs):
            calls.append((args, kwargs))
            if "list" in args:
                return b'[{"name":"projects/aidash-fixture/secrets/runtime/versions/1","state":"ENABLED"}]'
            if "access" in args:
                return json.dumps(raw).encode()
            return b""

        with patch("controller.run", side_effect=command):
            controller.provision_secret(
                {"project_id": "aidash-fixture"}, OUTPUT, "test"
            )
            additions = [kwargs for args, kwargs in calls if "add" in args]
            self.assertEqual(len(additions), 1)
            value = json.loads(additions[0]["data"])
            self.assertEqual(
                value["dashboard"]["gcip"]["tenant_bindings"], {"pool-a": "acme"}
            )
            self.assertEqual(value["node"]["api_token"], "private")
            raw.clear()
            raw.update(value)
            calls.clear()
            controller.provision_secret(
                {"project_id": "aidash-fixture"}, OUTPUT, "test"
            )
            self.assertFalse(any("add" in args for args, _ in calls))
            calls.clear()
            controller.provision_secret(
                {"project_id": "aidash-fixture", "gcip_web_api_key": "rotated"},
                OUTPUT,
                "test",
            )
            additions = [kwargs for args, kwargs in calls if "add" in args]
            self.assertEqual(len(additions), 1)
            self.assertEqual(
                json.loads(additions[0]["data"])["dashboard"]["gcip"]["web_api_key"],
                "rotated",
            )

    def test_oidc_and_gcip_runtime_configuration_cannot_coexist(self):
        with patch(
            "controller.run",
            side_effect=[
                b'[{"name":"projects/aidash-fixture/secrets/runtime/versions/1","state":"ENABLED"}]',
                b'{"dashboard":{"oidc":{"issuer":"issuer"}}}',
            ],
        ):
            with self.assertRaises(controller.Refused):
                controller.provision_secret(
                    {"project_id": "aidash-fixture"}, OUTPUT, "test"
                )

    def test_legacy_oidc_environment_cannot_coexist_with_gcip(self):
        with patch(
            "controller.run",
            side_effect=[
                b'[{"name":"projects/aidash-fixture/secrets/runtime/versions/1","state":"ENABLED"}]',
                b'{"AIDASH_OIDC_CLIENT_ID":"legacy"}',
            ],
        ):
            with self.assertRaisesRegex(controller.Refused, "Remove OIDC"):
                controller.provision_secret(
                    {"project_id": "aidash-fixture"}, OUTPUT, "test"
                )

    def test_refresh_uses_newest_enabled_version_after_a_rollback(self):
        versions = [
            {
                "name": f"projects/aidash-fixture/secrets/runtime/versions/{number}",
                "state": state,
            }
            for number, state in [
                (9, "ENABLED"),
                (12, "DESTROYED"),
                (10, "ENABLED"),
                (11, "DISABLED"),
            ]
        ]
        raw = {
            "node": {"api_token": "private"},
            "dashboard": {"gcip": {"web_api_key": "public"}},
        }
        published = []
        accessed = []

        def command(*args, **kwargs):
            if "list" in args:
                self.assertIn("--format=json(name,state)", args)
                return json.dumps(versions).encode()
            if "access" in args:
                version = args[args.index("access") + 1]
                self.assertEqual(version, "10")
                accessed.append(version)
                return json.dumps(raw).encode()
            if "add" in args:
                published.append(json.loads(kwargs["data"]))
            return b""

        with patch("controller.run", side_effect=command):
            controller.provision_secret(
                {"project_id": "aidash-fixture"}, OUTPUT, "test"
            )
        self.assertEqual(accessed, ["10"])
        self.assertEqual(len(published), 1)
        self.assertEqual(published[0]["node"]["api_token"], "private")
        self.assertEqual(
            published[0]["dashboard"]["gcip"]["tenant_bindings"], {"pool-a": "acme"}
        )

        # A disabled latest must be replaced even if its enabled predecessor
        # already has the desired bindings; the VM cannot list secret versions.
        raw.clear()
        raw.update(published[0])
        published.clear()
        accessed.clear()
        with patch("controller.run", side_effect=command):
            controller.provision_secret(
                {"project_id": "aidash-fixture"}, OUTPUT, "test"
            )
        self.assertEqual(accessed, ["10"])
        self.assertEqual(published, [raw])
