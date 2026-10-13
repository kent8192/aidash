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
import gcip
import controller

MEMBER = "serviceAccount:runtime@aidash-fixture.iam.gserviceaccount.com"
OUTPUT = {
    "runtime_secret": "runtime",
    "gcip": {
        "project_id": "aidash-fixture",
        "public_origin": "https://test.aidash.run",
        "auth_helper": "public_origin",
        "tenant_ids": ["pool-a"],
        "runtime_service_account": MEMBER.split(":", 1)[1],
        "tenant_bindings": {"pool-a": "acme"},
        "providers": {"pool-a": ["password"]},
        "password_sign_up": ["pool-a"],
        "sign_in_domains": {"acme.example": "pool-a"},
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
def declare(state, intervals=5):
    return {"state": state, "totp_adjacent_intervals": intervals}


def totp(state, intervals=5):
    return {
        "state": state,
        "enabledProviders": [],
        "providerConfigs": [{"state": "ENABLED", "totpProviderConfig": {"adjacentIntervals": intervals}}],
    }


DECLARED = dict(OUTPUT, gcip=dict(OUTPUT["gcip"], mfa={"pool-a": declare("disabled")}))


class AuthorizedDomainTests(unittest.TestCase):
    def test_random_domain_lifecycle_preserves_existing_domains_and_other_configuration(self):
        hostname = "0123456789abcdef" * 2 + ".preview.aidash.run"
        output = dict(OUTPUT, hostname=hostname, gcip=dict(OUTPUT["gcip"], auth_helper="firebase"))
        configuration = {
            "name": "projects/aidash-fixture/config",
            "authorizedDomains": ["aidash.run", "operator.example", "aidash-fixture.firebaseapp.com"],
            "signIn": {"email": {"enabled": False}},
        }
        original = deepcopy(configuration)
        writes = []

        def request(url, method, body=None):
            self.assertEqual(url.split("?")[0], "https://identitytoolkit.googleapis.com/admin/v2/projects/aidash-fixture/config")
            if method == "GET":
                self.assertIsNone(body)
                return deepcopy(configuration)
            self.assertEqual((method, url.split("?")[1]), ("PATCH", "updateMask=authorizedDomains"))
            self.assertEqual(set(body), {"authorizedDomains"})
            writes.append(deepcopy(body))
            configuration.update(body)
            return deepcopy(configuration)

        with patch("gcip.admin_request", side_effect=request):
            gcip.ensure_authorized_domain(output)
            gcip.ensure_authorized_domain(output)
            self.assertEqual(configuration["authorizedDomains"], original["authorizedDomains"] + [hostname])
            gcip.ensure_authorized_domain(output, enabled=False)
            gcip.ensure_authorized_domain(output, enabled=False)
        self.assertEqual(len(writes), 2)
        self.assertEqual(configuration, original)

    def test_random_domains_require_managed_firebase_auth_and_exact_hostname(self):
        hostname = "a" * 32 + ".preview.aidash.run"
        invalid = [
            {"hostname": hostname},
            dict(OUTPUT, hostname=hostname),
            dict(OUTPUT, hostname="*.preview.aidash.run"),
            dict(OUTPUT, hostname="aidash.run.preview.aidash.run"),
            dict(OUTPUT, hostname=hostname, gcip=dict(OUTPUT["gcip"], auth_helper="firebase", project_id="../other")),
        ]
        with patch("gcip.admin_request") as request:
            for output in invalid:
                with self.subTest(output=output), self.assertRaises(ValueError):
                    gcip.ensure_authorized_domain(output)
            gcip.ensure_authorized_domain(dict(OUTPUT, hostname="test.aidash.run"))
        request.assert_not_called()


class TenantMFATests(unittest.TestCase):
    def test_new_tenant_without_mfa_config_already_holds_the_disabled_requirement(self):
        api = FakeMFA({"name": "projects/aidash-fixture/tenants/pool-a"})
        self.assertEqual(dict(reconcile_mfa(DECLARED, api=api)), {})
        self.assertFalse(api.patches)

    def test_enabled_factors_are_drift_and_replaced_by_the_complete_declared_config(self):
        for actual, fields in (
            ({"state": "ENABLED", "enabledProviders": ["PHONE_SMS"]}, ["enabledProviders", "state"]),
            ({"state": "DISABLED", "providerConfigs": [{"state": "ENABLED", "totpProviderConfig": {"adjacentIntervals": 5}}]}, ["providerConfigs"]),
            ({"state": "MANDATORY"}, ["state"]),
        ):
            with self.subTest(actual=actual):
                api = FakeMFA({"mfaConfig": actual})
                self.assertEqual(dict(reconcile_mfa(DECLARED, api=api)), {"pool-a": fields})
                self.assertEqual(api.patches, [{"mfaConfig": DISABLED}])
                self.assertEqual(dict(reconcile_mfa(DECLARED, api=api)), {})
                self.assertEqual(len(api.patches), 1)

    def test_inert_provider_configs_are_not_drift(self):
        api = FakeMFA({"mfaConfig": {"state": "STATE_UNSPECIFIED", "enabledProviders": ["PROVIDER_UNSPECIFIED"], "providerConfigs": [{"state": "DISABLED", "totpProviderConfig": {"adjacentIntervals": 5}}]}})
        self.assertEqual(dict(reconcile_mfa(DECLARED, api=api)), {})
        self.assertFalse(api.patches)

    def test_state_without_a_declaration_is_left_for_its_next_apply(self):
        api = FakeMFA({"mfaConfig": {"state": "ENABLED"}})
        self.assertEqual(dict(reconcile_mfa(OUTPUT, api=api)), {})
        self.assertFalse(api.patches)

    def test_enabled_and_mandatory_requirements_hold_the_complete_totp_config(self):
        for state, intervals in (("enabled", 5), ("mandatory", 0), ("mandatory", 10)):
            output = dict(OUTPUT, gcip=dict(OUTPUT["gcip"], mfa={"pool-a": declare(state, intervals)}))
            with self.subTest(state=state, intervals=intervals):
                # A new tenant has no mfaConfig: every declared field is drift.
                api = FakeMFA({"name": "projects/aidash-fixture/tenants/pool-a"})
                self.assertEqual(dict(reconcile_mfa(output, api=api)), {"pool-a": ["providerConfigs", "state"]})
                self.assertEqual(api.patches, [{"mfaConfig": totp(state.upper(), intervals)}])
                self.assertEqual(dict(reconcile_mfa(output, api=api)), {})
                self.assertEqual(len(api.patches), 1)

    def test_requirement_changes_and_factor_drift_are_corrected(self):
        enabled = dict(OUTPUT, gcip=dict(OUTPUT["gcip"], mfa={"pool-a": declare("enabled", 3)}))
        for actual, fields in (
            (totp("MANDATORY", 3), ["state"]),
            (totp("ENABLED", 5), ["providerConfigs"]),
            ({"state": "ENABLED", "enabledProviders": ["PHONE_SMS"], "providerConfigs": totp("ENABLED", 3)["providerConfigs"]}, ["enabledProviders"]),
            ({"state": "ENABLED", "providerConfigs": [{"state": "DISABLED", "totpProviderConfig": {"adjacentIntervals": 3}}]}, ["providerConfigs"]),
            (dict(totp("ENABLED", 3), providerConfigs=[]), ["providerConfigs"]),
        ):
            with self.subTest(actual=actual):
                api = FakeMFA({"mfaConfig": actual})
                self.assertEqual(dict(reconcile_mfa(enabled, api=api)), {"pool-a": fields})
                self.assertEqual(api.patches, [{"mfaConfig": totp("ENABLED", 3)}])
        # Lowering a Requirement to disabled removes the TOTP provider config entirely.
        api = FakeMFA({"mfaConfig": totp("MANDATORY")})
        self.assertEqual(dict(reconcile_mfa(DECLARED, api=api)), {"pool-a": ["providerConfigs", "state"]})
        self.assertEqual(api.patches, [{"mfaConfig": DISABLED}])

    def test_unsupported_requirement_is_refused_before_any_write(self):
        for declared in (
            "mandatory",
            declare("required"),
            declare("enabled", 11),
            declare("enabled", -1),
            declare("enabled", 2.5),
            declare("enabled", True),
            {"state": "enabled"},
        ):
            with self.subTest(declared=declared):
                api = FakeMFA({"mfaConfig": {"state": "ENABLED"}})
                output = dict(OUTPUT, gcip=dict(OUTPUT["gcip"], mfa={"pool-a": declared}))
                with self.assertRaisesRegex(ValueError, "Unsupported MFA Requirement"):
                    dict(reconcile_mfa(output, api=api))
                self.assertFalse(api.patches)

    def test_a_later_tenant_failure_cannot_hide_an_earlier_correction(self):
        class LaterFailure(FakeMFA):
            def call(self, method, resource, body=None):
                if resource.startswith("projects/aidash-fixture/tenants/pool-b"):
                    raise HTTPError("", 503, "unavailable", {}, None)
                return super().call(method, resource, body)

        api = LaterFailure({"mfaConfig": {"state": "ENABLED"}})
        output = dict(OUTPUT, gcip=dict(OUTPUT["gcip"], tenant_ids=["pool-a", "pool-b"], mfa={"pool-a": declare("disabled"), "pool-b": declare("disabled")}))
        corrections = reconcile_mfa(output, api=api)
        # The caller records pool-a's correction before pool-b is touched.
        self.assertEqual(next(corrections), ("pool-a", ["state"]))
        self.assertEqual(api.patches, [{"mfaConfig": DISABLED}])
        with self.assertRaises(HTTPError):
            next(corrections)

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
            TenantMFA().reconcile("aidash-fixture", "pool-a", declare("disabled"))
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

    def test_mandatory_pools_reach_the_runtime_as_a_sign_in_hint(self):
        raw = {"dashboard": {"gcip": {"web_api_key": "public"}}}

        def command(*args, **kwargs):
            if "list" in args:
                return b'[{"name":"projects/aidash-fixture/secrets/runtime/versions/1","state":"ENABLED"}]'
            if "access" in args:
                return json.dumps(raw).encode()
            if "add" in args:
                raw.clear()
                raw.update(json.loads(kwargs["data"]))
            return b""

        def hint(mfa):
            output = deepcopy(OUTPUT)
            if mfa is not None:
                output["gcip"]["mfa"] = mfa
            controller.provision_secret({"project_id": "aidash-fixture"}, output, "test")
            return raw["dashboard"]["gcip"]["mfa_mandatory"]

        with patch("controller.run", side_effect=command):
            # State written before tenant MFA existed lists none.
            self.assertEqual(hint(None), [])
            self.assertEqual(hint({"pool-a": declare("enabled")}), [])
            self.assertEqual(hint({"pool-a": declare("mandatory")}), ["pool-a"])
            self.assertEqual(hint({"pool-a": declare("disabled")}), [])

    def test_auth_helper_follows_the_applied_release_output(self):
        raw = {"dashboard": {"gcip": {"web_api_key": "public"}}}

        def command(*args, **kwargs):
            if "list" in args:
                return b'[{"name":"projects/aidash-fixture/secrets/runtime/versions/1","state":"ENABLED"}]'
            if "access" in args:
                return json.dumps(raw).encode()
            if "add" in args:
                raw.clear()
                raw.update(json.loads(kwargs["data"]))
            return b""

        firebase = deepcopy(OUTPUT)
        firebase["gcip"]["auth_helper"] = "firebase"
        with patch("controller.run", side_effect=command):
            # A refresh reloads a release deployed before the edge proxied the
            # helper; the default needs no key.
            controller.provision_secret({"project_id": "aidash-fixture"}, firebase, "test")
            self.assertNotIn("auth_helper", raw["dashboard"]["gcip"])
            self.assertEqual(raw["dashboard"]["gcip"]["tenant_bindings"], {"pool-a": "acme"})
            controller.provision_secret({"project_id": "aidash-fixture"}, OUTPUT, "test")
            self.assertEqual(raw["dashboard"]["gcip"]["auth_helper"], "public_origin")
            controller.provision_secret({"project_id": "aidash-fixture"}, firebase, "test")
            self.assertNotIn("auth_helper", raw["dashboard"]["gcip"])

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
        # already has the desired bindings; readers of "latest" must see it.
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
