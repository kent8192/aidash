"""Independent adoption, secret delivery and saved-plan safety contracts."""

import json
import io
from copy import deepcopy
import os
from pathlib import Path
import stat
import sys
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "control"))
import setup


def environment(name):
    value = {"can_admins_bypass": False,
             "deployment_branch_policy": {"protected_branches": False, "custom_branch_policies": True},
             "protection_rules": []}
    if name == "production":
        value["protection_rules"] = [{"type": "required_reviewers", "prevent_self_review": True,
                                       "reviewers": [{"type": "User", "reviewer": {"id": 51869472}}]}]
    return value


class SetupTests(unittest.TestCase):
    def setUp(self):
        self.addCleanup(patch.stopall)
        self.api = {"repos/kent8192/aidash": {"full_name": setup.REPOSITORY, "default_branch": "develop/0.1.0"},
                    "repos/kent8192/aidash/rulesets?per_page=100&page=1": [{"id": 23727469, "name": "main"}, {"id": 23727567, "name": "develop/x.y.z"}],
                    "repos/kent8192/aidash/environments?per_page=100&page=1": {"environments": []},
                    "repos/kent8192/aidash/actions/variables?per_page=100&page=1": {"variables": [{"name": "GCP_ENVIRONMENTS_ENABLED", "value": "false"}, {"name": "USER_SETTING", "value": "retained"}]}}
        for name, branch in setup.ENVIRONMENTS.items():
            prefix = f"repos/kent8192/aidash/environments/{name}"
            self.api[prefix] = environment(name)
            self.api[prefix + "/deployment-branch-policies?per_page=100&page=1"] = {"branch_policies": [{"id": 100, "name": branch, "type": "branch"}]}
        patch.object(setup, "github", side_effect=lambda path: self.api[path]).start()

    def test_unrelated_rulesets_and_variables_are_never_adopted(self):
        adoption = setup.github_adoption()
        self.assertEqual(adoption["existing_delivery_ruleset_ids"], {})
        self.assertEqual(adoption["existing_repository_variables"], ["GCP_ENVIRONMENTS_ENABLED"])
        self.assertNotIn("USER_SETTING", json.dumps(adoption))

    def test_ambiguous_owned_rule_or_active_workflow_blocks_setup(self):
        rules = self.api["repos/kent8192/aidash/rulesets?per_page=100&page=1"]
        rules.extend([{"id": 1, "name": setup.RULESETS["main"]}, {"id": 2, "name": setup.RULESETS["main"]}])
        with self.assertRaises(setup.Refused):
            setup.github_adoption()
        rules.pop()
        self.api["repos/kent8192/aidash/rulesets/1"] = {"name": setup.RULESETS["main"], "target": "branch", "enforcement": "active",
            "conditions": {"ref_name": {"include": ["refs/heads/main"], "exclude": []}},
            "rules": [{"type": "required_status_checks", "parameters": {"required_status_checks": [{"context": "CI Success", "integration_id": 15368}]}}]}
        self.api["repos/kent8192/aidash/actions/variables?per_page=100&page=1"]["variables"][0]["value"] = "true"
        with self.assertRaises(setup.Refused):
            setup.github_adoption()

    def test_existing_owned_name_with_additional_controls_is_never_adopted(self):
        self.api["repos/kent8192/aidash/rulesets?per_page=100&page=1"].append({"id": 1, "name": setup.RULESETS["main"]})
        self.api["repos/kent8192/aidash/rulesets/1"] = {"name": setup.RULESETS["main"], "target": "branch", "enforcement": "active",
            "conditions": {"ref_name": {"include": ["refs/heads/main"], "exclude": []}},
            "rules": [{"type": "pull_request"}]}
        with self.assertRaises(setup.Refused):
            setup.github_adoption()

    def test_extra_tag_or_wildcard_policy_blocks_secrets_before_any_write(self):
        values = {"nonproduction": {"CLOUDFLARE_DNS_API_TOKEN": "private-dns-token"}, "production": {"GCP_PRODUCTION_RUNTIME_CONFIG": "private-runtime"}}
        policies = self.api["repos/kent8192/aidash/environments/production/deployment-branch-policies?per_page=100&page=1"]["branch_policies"]
        policies.append({"id": 101, "name": "*", "type": "tag"})
        with patch.object(setup, "command") as writes, self.assertRaises(setup.Refused):
            setup.install_secrets(values)
        writes.assert_not_called()

    def test_production_reviewer_self_approval_and_admin_bypass_are_rejected(self):
        target = self.api["repos/kent8192/aidash/environments/production"]
        target["can_admins_bypass"] = True
        with self.assertRaises(setup.Refused):
            setup.verify_environment("production", {51869472})
        target["can_admins_bypass"] = False
        target["protection_rules"][0]["prevent_self_review"] = False
        with self.assertRaises(setup.Refused):
            setup.verify_environment("production", {51869472})

    def test_secrets_are_only_sent_to_fixed_protected_environments_via_stdin(self):
        secret = "private-token-that-must-not-be-in-argv"
        with patch.object(setup, "command", return_value=b"") as send:
            result = setup.install_secrets({"deployment-notifications": {"CLOUDFLARE_EMAIL_SENDING_TOKEN": secret}})
        self.assertEqual(result, {"protected_secrets_installed": 1})
        self.assertNotIn(secret, " ".join(send.call_args.args[0]))
        self.assertEqual(send.call_args.kwargs["data"], secret.encode())
        self.assertIn("deployment-notifications", send.call_args.args[0])
        with self.assertRaises(setup.Refused):
            setup.install_secrets({"production": {"CLOUDFLARE_EMAIL_SENDING_TOKEN": secret}})

    def test_plan_output_hides_values_and_refuses_any_deletion_or_replacement(self):
        change = {"mode": "managed", "address": "github_actions_variable.delivery[\"GCP_ENVIRONMENTS_ENABLED\"]",
                  "change": {"actions": ["update"], "after": {"value": "private-input"}}}
        summary = setup.plan_changes({"resource_changes": [change]})
        self.assertNotIn("private-input", json.dumps(summary))
        for actions in [["delete"], ["delete", "create"], ["create", "delete"]]:
            change["change"]["actions"] = actions
            with self.assertRaises(setup.Refused):
                setup.plan_changes({"resource_changes": [change]})

    def test_private_operator_files_are_not_world_readable_and_refuse_symlinks(self):
        with TemporaryDirectory() as directory:
            path = Path(directory) / "operator.json"
            setup.private_json(path, {"private": "input"})
            self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o600)
            link = Path(directory) / "link.json"
            os.symlink(path, link)
            with self.assertRaises(setup.Refused):
                setup.private_json(link, {})

    def test_reviewed_migration_never_revokes_another_identity_or_destroys_resources(self):
        project = "aidash-nonproduction"
        binding = {"mode": "managed", "type": "google_project_iam_member", "address": 'google_project_iam_member.deploy["roles/compute.securityAdmin"]',
                   "change": {"actions": ["delete"], "before": {"role": "roles/compute.securityAdmin", "project": project,
                              "member": f"serviceAccount:aidash-deploy@{project}.iam.gserviceaccount.com"}, "after": None}}
        with self.assertRaises(setup.Refused):
            setup.plan_changes({"resource_changes": [binding]})
        self.assertEqual(len(setup.plan_changes({"resource_changes": [binding]}, project)), 1)
        binding["change"]["before"]["member"] = "user:another-owner@example.test"
        with self.assertRaises(setup.Refused):
            setup.plan_changes({"resource_changes": [binding]}, project)
        binding["type"] = "google_storage_bucket"
        with self.assertRaises(setup.Refused):
            setup.plan_changes({"resource_changes": [binding]}, project)

    def account_inputs(self, root, extra=None):
        values = {"release_app_file": str(root / "app.json"), "cloudflare_zone_id": "b" * 32,
                  "production_reviewer_user_ids": [51869472], "gcip_tenants": {tier: {"default": {"tenant": "public-pool"}} for tier in ("nonproduction", "production")}}
        setup.private_json(root / "app.json", {"release_app": {"id": 12, "client_id": "Iv23.fixture", "slug": "aidash-delivery", "installation_id": 34}})
        for tier in ("nonproduction", "production"):
            path = root / (tier + ".json")
            values[tier + "_output_file"] = str(path)
            config = {"project_id": "aidash-" + tier, "deployment_tier": tier, "state_bucket": tier + "-state", "release_bucket": tier + "-release",
                      "notification_bucket": tier + "-notifications", "deploy_service_account": tier + "-deploy", "notification_service_account": tier + "-notify",
                      "registry": "bootstrap-only-registry", "production_node_service_account": "bootstrap-only-node", "broker_signing_keys": {"production": "bootstrap-only-key"}}
            output = {"configuration": {"value": config}, "operations_configuration": {"value": {"gke_backup_enabled": tier == "production"}},
                      "gcip_web_api_key": {"value": "public-" + tier + "-browser-key", "sensitive": True},
                      "preview_tls_renewal": {"value": {"secret_id": "preview-tls", "acme_state_secret_id": "acme-state", "service_account": "renewer", "workload_identity_provider": "wif"}}}
            if extra is not None:
                extra(tier, output)
            setup.private_json(path, output)
        return values

    def test_composition_filters_bootstrap_only_fields_and_keeps_activation_disabled(self):
        with TemporaryDirectory() as directory:
            values = self.account_inputs(Path(directory))
            result = setup.compose(values)
            for tier in ("nonproduction", "production"):
                public = json.loads(result[tier + "_config_json"])
                self.assertNotIn("registry", public)
                self.assertNotIn("production_node_service_account", public)
                self.assertNotIn("broker_signing_keys", public)
                self.assertNotIn("credential_brokers", public)
                self.assertEqual(public["gcip_tenants"], {"default": {"tenant": "public-pool"}})
                # Without it the first GCIP deployment cannot provision its runtime secret.
                self.assertEqual(public["gcip_web_api_key"], "public-" + tier + "-browser-key")
            self.assertNotIn("enablement", result)
            self.assertNotIn("activation_validated", result)
            self.assertNotIn("require_staging_check", result)
            self.assertTrue(json.loads(result["production_config_json"])["gke_backup_enabled"])
            production_output = Path(values["production_output_file"])
            without_key = json.loads(production_output.read_text())
            del without_key["gcip_web_api_key"]
            production_output.unlink()
            setup.private_json(production_output, without_key)
            with self.assertRaisesRegex(setup.Refused, "gcip_web_api_key"):
                setup.compose(values)
            values["gcip_tenants"]["production"] = {}
            with self.assertRaises(setup.Refused):
                setup.compose(values)

    def test_composition_preserves_bootstrap_brokers_and_operator_enablement(self):
        def brokers(tier, output):
            identity = "production" if tier == "production" else "test"
            output["configuration"]["value"].update(byok_project_id="aidash-byok", broker_service_accounts={identity: f"aidash-{identity}-broker@{tier}"})
            output["broker_signing_keys"] = {"value": {identity: f"projects/{tier}/keyRings/aidash-{identity}-capability/cryptoKeys/capability"}}
        with TemporaryDirectory() as directory:
            values = self.account_inputs(Path(directory), brokers)
            values["credential_brokers"] = {"nonproduction": {"test": {"enabled": True, "signing_version": "2", "verification_versions": ["1", "2"]}}}
            result = setup.compose(values)
            self.assertEqual(json.loads(result["nonproduction_config_json"])["credential_brokers"], {"test": {
                "enabled": True, "signing_version": "2", "verification_versions": ["1", "2"], "byok_project_id": "aidash-byok", "secret_prefix": "aidash-test-cred-",
                "broker_service_account_email": "aidash-test-broker@nonproduction",
                "signing_key_id": "projects/nonproduction/keyRings/aidash-test-capability/cryptoKeys/capability"}})
            # Unconfigured bootstrap brokers stay published but disabled, so reconcile never drains them by omission.
            self.assertFalse(json.loads(result["production_config_json"])["credential_brokers"]["production"]["enabled"])
            for settings in ({"develop": {"enabled": True}}, {"test": {"image": "attacker.invalid/broker:latest"}}, {"test": {"enabled": "true"}}):
                with self.subTest(settings=settings):
                    values["credential_brokers"] = {"nonproduction": settings}
                    with self.assertRaises(setup.Refused):
                        setup.compose(values)

    def test_production_state_relocation_preserves_lineage_and_cannot_overwrite_existing_state(self):
        from types import SimpleNamespace
        for scenario in ("valid", "occupied", "different-lineage"):
            with self.subTest(scenario=scenario), TemporaryDirectory() as directory:
                root = Path(directory)
                before = {"lineage": "owned-production-lineage", "resources": [], "outputs": {"configuration": {"value": {
                    "deployment_tier": "production", "project_id": "aidash-production", "nonproduction_project_id": "aidash-nonproduction", "state_bucket": "aidash-production-state"}}}}
                after = deepcopy(before)
                if scenario == "different-lineage":
                    after["lineage"] = "unrelated-lineage"
                commands, requests = [], []
                states = iter([before, after])
                def run(*arguments, commands=commands, states=states):
                    commands.append(arguments)
                    return json.dumps(next(states)).encode() if arguments == ("state", "pull") else b""
                def request(operation, timeout=30, requests=requests, scenario=scenario):
                    requests.append((operation.get_method(), operation.full_url))
                    if operation.get_method() == "GET" and "/b/aidash-production-state/" in operation.full_url and scenario != "occupied":
                        raise setup.urllib.error.HTTPError(operation.full_url, 404, "missing", {}, None)
                    return io.BytesIO(json.dumps({"generation": "123"}).encode())
                stack = SimpleNamespace(name="production", bucket="aidash-initial-state", prefix="terraform/delivery/production", directory=root,
                                        plan=root / "reviewed.tfplan", environment={"GOOGLE_OAUTH_ACCESS_TOKEN": "private-token"},
                                        initialize=lambda: None, run=run)
                with patch.object(setup.urllib.request, "urlopen", request):
                    if scenario == "valid":
                        self.assertTrue(setup.relocate_production_state(stack, "aidash-production-state")["lineage_preserved"])
                    else:
                        with self.assertRaises(setup.Refused):
                            setup.relocate_production_state(stack, "aidash-production-state")
                deletes = [url for method, url in requests if method == "DELETE"]
                self.assertEqual(len(deletes), int(scenario == "valid"))
                if deletes:
                    self.assertIn("ifGenerationMatch=123", deletes[0])
                    self.assertIn("/b/aidash-initial-state/", deletes[0])
                if scenario == "occupied":
                    self.assertFalse(any("-migrate-state" in command for command in commands))

    def test_partial_project_cleanup_requires_the_exact_owned_empty_default_network(self):
        values = {"production_project_id": "aidash-production", "billing_account_id": "000000-000000-000000"}
        state = {"resources": [{"type": "google_project", "name": "production", "instances": [{"status": "tainted", "attributes": {"project_id": "aidash-production"}}]}]}
        project = {"projectId": "aidash-production", "state": "ACTIVE", "labels": {"application": "aidash", "environment": "production"}}
        billing = {"billingEnabled": True, "billingAccountName": "billingAccounts/000000-000000-000000"}
        network = {"name": "default", "description": "Default network for the project", "autoCreateSubnetworks": True, "selfLink": "owned-default"}
        firewalls = {"items": [{"name": "default-allow-ssh", "network": "owned-default"}]}
        original = [values, state, project, billing, network, firewalls, {"items": {}}]
        self.assertEqual(setup.project_repair_allowed(*original), ["default-allow-ssh"])
        for index, changed in [(2, dict(project, labels={})), (3, dict(billing, billingEnabled=False)), (4, dict(network, peerings=["foreign-network"])),
                               (5, {"items": [{"name": "user-rule", "network": "owned-default"}]}), (6, {"items": {"zone": {"instances": [{"name": "running"}]}}})]:
            inputs = deepcopy(original)
            inputs[index] = changed
            with self.subTest(index=index), self.assertRaises(setup.Refused):
                setup.project_repair_allowed(*inputs)


if __name__ == "__main__":
    unittest.main()
