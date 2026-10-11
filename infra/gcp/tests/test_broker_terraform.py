"""Inspect real Terraform state produced exclusively by the mock Google provider."""
import json
from pathlib import Path
import shutil
import subprocess
import unittest

MODULE = Path(__file__).resolve().parents[1] / "modules" / "credential-broker"


@unittest.skipUnless(shutil.which("terraform"), "terraform is required")
class CredentialBrokerTerraformTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        def flattened(module):
            return module.get("resources", []) + [r for child in module.get("child_modules", []) for r in flattened(child)]

        def mocked(root, test_filter=None):
            subprocess.run(["terraform", f"-chdir={root}", "init", "-backend=false", "-input=false", "-no-color"], check=True, capture_output=True, timeout=120)
            command = ["terraform", f"-chdir={root}", "test", "-json", "-verbose", "-no-color"]
            if test_filter:
                command.append("-filter=" + test_filter)
            result = subprocess.run(command, capture_output=True, text=True, timeout=120)
            if result.returncode:
                raise AssertionError(result.stdout + result.stderr)
            states, plans = {}, {}
            for line in result.stdout.splitlines():
                event = json.loads(line)
                if event.get("type") == "test_state":
                    states[event["@testrun"]] = flattened(event["test_state"]["root_module"])
                elif event.get("type") == "test_plan":
                    plans[event["@testrun"]] = event["test_plan"].get("resource_changes", [])
            if not states:
                raise AssertionError("Terraform returned no mock test state")
            return states, plans
        cls.states, _ = mocked(MODULE)
        cls.bootstrap_states, _ = mocked(MODULE.parents[1] / "bootstrap")
        cls.lifecycle_states, cls.lifecycle_plans = mocked(MODULE.parents[1] / "environments", "tests/key_lifecycle.tftest.hcl")
        cls.environment_states, _ = mocked(MODULE.parents[1] / "environments", "tests/policy.tftest.hcl")

    def test_node_identity_holds_only_telemetry_and_image_pull(self):
        resources = self.environment_states["identities_are_separated"]
        grants = [r["values"] for r in resources if r["type"].endswith(("_iam_member", "_iam_binding"))]
        nodes = "serviceAccount:aidash-gke-nodes@aidash-fixture.iam.gserviceaccount.com"
        self.assertEqual(sorted(g["role"] for g in grants if g["member"] == nodes), [
            "roles/artifactregistry.reader", "roles/container.defaultNodeServiceAccount",
        ])
        workloads = sorted((g["role"], g["member"]) for g in grants if g["member"].startswith(("serviceAccount:server@", "serviceAccount:worker@")))
        self.assertEqual(workloads, [
            ("projects/aidash-byok-fixture/roles/aidashByokCreate", "serviceAccount:server@aidash-fixture.iam.gserviceaccount.com"),
            ("projects/aidash-byok-fixture/roles/aidashByokManage", "serviceAccount:server@aidash-fixture.iam.gserviceaccount.com"),
            ("roles/cloudkms.signer", "serviceAccount:worker@aidash-fixture.iam.gserviceaccount.com"),
        ])

    def resources(self, run, kind):
        return [r["values"] for r in self.states[run] if r["type"] == kind]

    def test_broker_sa_and_byok_read_grant_are_owned_exclusively_by_bootstrap(self):
        for run in ("production", "staging"):
            self.assertFalse(self.resources(run, "google_service_account"))
            self.assertFalse(self.resources(run, "google_project_iam_custom_role"))
            self.assertFalse(self.resources(run, "google_project_iam_member"))
            # No managed resource in this module may mutate the BYOK project.
            self.assertFalse([r for r in self.states[run] if r["mode"] == "managed" and r["values"].get("project") == "aidash-byok-fixture"])
            environment = "production" if run == "production" else "test"
            account = f"aidash-{environment}-broker@aidash-fixture.iam.gserviceaccount.com"
            service, = self.resources(run, "google_cloud_run_v2_service")
            self.assertEqual(service["template"][0]["service_account"], account)
            deploy, = self.resources(run, "google_service_account_iam_member")
            self.assertEqual(deploy["service_account_id"], f"projects/aidash-fixture/serviceAccounts/{account}")
            self.assertEqual(deploy["role"], "roles/iam.serviceAccountUser")
            self.assertEqual(deploy["member"], "serviceAccount:deploy@aidash-fixture.iam.gserviceaccount.com")

    def test_bootstrap_broker_bindings_are_the_only_byok_payload_accessors(self):
        resources = self.bootstrap_states["broker_environments"]
        roles = {r["values"]["name"]: r["values"] for r in resources if r["type"] == "google_project_iam_custom_role"}
        readers = [r["values"] for r in resources if r["type"] == "google_project_iam_member" and "secretmanager.versions.access" in roles.get(r["values"]["role"], {}).get("permissions", [])]
        self.assertEqual(len(readers), 2)
        self.assertEqual({r["member"] for r in readers}, {f"serviceAccount:aidash-{env}-broker@aidash-fixture.iam.gserviceaccount.com" for env in ("production", "test")})
        for grant in readers:
            self.assertEqual(grant["project"], "aidash-byok-fixture")
            environment = "production" if "production-broker@" in grant["member"] else "test"
            self.assertEqual(grant["condition"][0]["expression"], f'resource.name.startsWith("projects/123456789012/secrets/aidash-{environment}-cred-")')
        disabled = self.bootstrap_states["brokers_opt_in_only"]
        self.assertFalse([r for r in disabled if r["address"].startswith("google_service_account.broker[") or r["address"].startswith("google_project_iam_member.byok_broker_read[")])

    def test_bootstrap_deploy_can_read_public_keys_without_a_signing_grant(self):
        resources = self.bootstrap_states["broker_environments"]
        keys = {r["values"]["id"] for r in resources if r["type"] == "google_kms_crypto_key"}
        grants = [r["values"] for r in resources if r["type"] == "google_kms_crypto_key_iam_member"]
        self.assertEqual(len(grants), 2)
        self.assertEqual({grant["crypto_key_id"] for grant in grants}, keys)
        self.assertTrue(all(grant["role"] == "roles/cloudkms.publicKeyViewer" and grant["member"] == "serviceAccount:aidash-deploy@aidash-fixture.iam.gserviceaccount.com" for grant in grants))
        disabled = self.bootstrap_states["brokers_opt_in_only"]
        self.assertFalse([r for r in disabled if r["type"] == "google_kms_crypto_key_iam_member"])

    def test_runtime_has_only_key_scoped_signing_and_no_broker_role(self):
        for run in ("production", "staging"):
            signer, = self.resources(run, "google_kms_crypto_key_iam_member")
            environment = "production" if run == "production" else "test"
            keys = {r["values"]["id"]: r["values"] for r in self.bootstrap_states["broker_environments"] if r["type"] == "google_kms_crypto_key"}
            key = keys[signer["crypto_key_id"]]
            self.assertEqual(signer["crypto_key_id"], f"projects/aidash-fixture/locations/us-central1/keyRings/aidash-{environment}-capability/cryptoKeys/capability")
            self.assertFalse(self.resources(run, "google_kms_crypto_key"))
            self.assertFalse(self.resources(run, "google_kms_key_ring"))
            self.assertEqual(signer["role"], "roles/cloudkms.signer")
            self.assertEqual(signer["member"], "serviceAccount:runtime@aidash-fixture.iam.gserviceaccount.com")
            self.assertEqual(key["purpose"], "ASYMMETRIC_SIGN")
            self.assertEqual(key["version_template"][0]["algorithm"], "EC_SIGN_ED25519")
            self.assertFalse(self.resources(run, "google_cloud_run_v2_service_iam_member"))
            self.assertTrue(all("runtime@" not in g["member"] for g in self.resources(run, "google_project_iam_member")))

    def test_disable_reenable_and_retirement_plans_never_destroy_key_material(self):
        material = {"google_kms_key_ring", "google_kms_crypto_key", "google_kms_crypto_key_version"}
        for run in ("broker_disabled_plan", "broker_reenabled_plan", "environment_removed_plan"):
            changes = self.lifecycle_plans[run]
            self.assertFalse([r for r in changes if r["type"] in material])
            service, = [r for r in changes if r["type"] == "google_cloud_run_v2_service"]
            self.assertEqual(service["change"]["actions"], ["create"] if run == "broker_reenabled_plan" else ["delete"])
        permanent = [r for r in self.lifecycle_plans["bootstrap_after_retirement_plan"] if r["type"] in material]
        self.assertEqual({r["type"] for r in permanent}, {"google_kms_key_ring", "google_kms_crypto_key"})
        self.assertTrue(all(r["change"]["actions"] == ["no-op"] for r in permanent))

    def test_internal_ingress_uses_capability_auth_and_injected_public_keys(self):
        for run in ("production", "staging"):
            service, = self.resources(run, "google_cloud_run_v2_service")
            self.assertEqual(service["ingress"], "INGRESS_TRAFFIC_INTERNAL_ONLY")
            self.assertTrue(service["invoker_iam_disabled"])
            template, = service["template"]
            self.assertEqual(template["timeout"], "3600s")
            self.assertEqual(template["scaling"][0]["max_instance_count"], 10)
            self.assertEqual(template["scaling"][0]["min_instance_count"], 1 if run == "production" else 0)
            self.assertFalse(template["vpc_access"])
            env = {v["name"]: v["value"] for v in template["containers"][0]["env"]}
            self.assertEqual(env["AIDASH_BROKER_TIMEOUT_SECS"], "3570")
            keys = json.loads(env["AIDASH_CAPABILITY_PUBLIC_KEYS"])
            self.assertEqual(list(keys.values()), ["PUBLIC-KEY-FIXTURE"])
            self.assertTrue(next(iter(keys)).endswith("/cryptoKeyVersions/1"))

    def test_managed_rotation_updates_service_without_replacing_key_material(self):
        changes = self.lifecycle_plans["broker_rotation_plan"]
        material = {"google_kms_key_ring", "google_kms_crypto_key", "google_kms_crypto_key_version"}
        self.assertFalse([r for r in changes if r["mode"] == "managed" and r["type"] in material])
        service, = [r for r in changes if r["type"] == "google_cloud_run_v2_service"]
        self.assertEqual(service["change"]["actions"], ["update"])
        signer, = [r for r in changes if r["type"] == "google_kms_crypto_key_iam_member"]
        self.assertEqual(signer["change"]["actions"], ["no-op"])
        key = signer["change"]["after"]["crypto_key_id"]
        env = {item["name"]: item["value"] for item in service["change"]["after"]["template"][0]["containers"][0]["env"]}
        self.assertEqual(json.loads(env["AIDASH_CAPABILITY_PUBLIC_KEYS"]), {
            f"{key}/cryptoKeyVersions/1": "fixture-public-key",
            f"{key}/cryptoKeyVersions/2": "fixture-public-key-2",
        })

    def test_pr_and_disabled_environments_have_no_broker_or_accessor(self):
        for run in ("pr", "disabled"):
            self.assertFalse([r for r in self.states[run] if r["mode"] == "managed"])


if __name__ == "__main__":
    unittest.main()
