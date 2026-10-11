"""#151: the deploy identity cannot reach the Credential Broker identity.

Terraform state comes exclusively from the mock Google provider; nothing here
touches Google Cloud.
"""
import json
from pathlib import Path
import re
import shutil
import subprocess
import unittest

import yaml

BOOTSTRAP = Path(__file__).resolve().parents[1] / "bootstrap"
WORKFLOWS = Path(__file__).resolve().parents[3] / ".github" / "workflows"
DEPLOY = "serviceAccount:aidash-deploy@aidash-fixture.iam.gserviceaccount.com"
IAM_GRANTS = (
    "google_project_iam_member", "google_project_iam_binding",
    "google_service_account_iam_member", "google_service_account_iam_binding",
    "google_storage_bucket_iam_member", "google_artifact_registry_repository_iam_member",
    "google_kms_crypto_key_iam_member",
)
# IAM v1 names of the permissions #151 forbids on broker accounts, and the deny-policy
# names that must cover each of them.
SENSITIVE = {
    "iam.serviceAccounts.setIamPolicy": "iam.googleapis.com/serviceAccounts.setIamPolicy",
    "iam.serviceAccounts.getAccessToken": "iam.googleapis.com/serviceAccounts.getAccessToken",
    "iam.serviceAccounts.signBlob": "iam.googleapis.com/serviceAccounts.signBlob",
    "iam.serviceAccounts.signJwt": "iam.googleapis.com/serviceAccounts.signJwt",
    "iam.serviceAccounts.implicitDelegation": "iam.googleapis.com/serviceAccounts.implicitDelegation",
    "iam.serviceAccountKeys.create": "iam.googleapis.com/serviceAccountKeys.create",
}
# Every predefined role the deploy identity may hold, with the sensitive permissions it
# carries. A role missing here fails the test: whoever adds it must decide whether it
# can reach a broker account (for example roles/iam.serviceAccountTokenCreator or
# roles/iam.serviceAccountKeyAdmin would) and extend the deny policy first.
PREDEFINED_ROLES = {
    "roles/iam.serviceAccountAdmin": {"iam.serviceAccounts.setIamPolicy"},
    # actAs only; deliberately not denied (Cloud Run needs it, residual path 2).
    "roles/iam.serviceAccountUser": set(),
    "roles/compute.instanceAdmin.v1": set(), "roles/compute.networkAdmin": set(),
    "roles/container.admin": set(), "roles/secretmanager.admin": set(),
    "roles/artifactregistry.reader": set(), "roles/artifactregistry.writer": set(),
    "roles/artifactregistry.admin": set(), "roles/serviceusage.serviceUsageConsumer": set(),
    "roles/run.admin": set(), "roles/cloudkms.admin": set(), "roles/cloudkms.publicKeyViewer": set(),
    # Project IAM administration is limited by a modifiedGrantsByRole condition to the
    # GKE node role, which carries no impersonation permission.
    "roles/resourcemanager.projectIamAdmin": set(),
    "roles/storage.objectAdmin": set(), "roles/storage.admin": set(),
}


def flattened(module):
    return module.get("resources", []) + [r for child in module.get("child_modules", []) for r in flattened(child)]


def uncovered(roles, custom_roles, denied):
    """Return sensitive permissions deploy holds through `roles` that `denied` misses."""
    held = set()
    for role in roles:
        if role in PREDEFINED_ROLES:
            held |= PREDEFINED_ROLES[role]
        elif role in custom_roles:
            held |= set(custom_roles[role]) & set(SENSITIVE)
        else:
            raise AssertionError(f"Classify {role}: can it reach a Credential Broker account?")
    return sorted(permission for permission in held if SENSITIVE[permission] not in denied)


@unittest.skipUnless(shutil.which("terraform"), "terraform is required")
class DeployBoundaryTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        subprocess.run(["terraform", f"-chdir={BOOTSTRAP}", "init", "-backend=false", "-input=false", "-no-color"], check=True, capture_output=True, timeout=300)
        result = subprocess.run(
            ["terraform", f"-chdir={BOOTSTRAP}", "test", "-json", "-verbose", "-no-color", "-filter=tests/deploy_boundary.tftest.hcl"],
            capture_output=True, text=True, timeout=600,
        )
        if result.returncode:
            raise AssertionError(result.stdout + result.stderr)
        states = {}
        for line in result.stdout.splitlines():
            event = json.loads(line)
            if event.get("type") == "test_state":
                states[event["@testrun"]] = flattened(event["test_state"]["root_module"])
        cls.resources = states["deploy_boundary"]

    def values(self, kind):
        return [r["values"] for r in self.resources if r["type"] == kind]

    def deny_rule(self):
        policy, = self.values("google_iam_deny_policy")
        rule, = policy["rules"][0]["deny_rule"]
        return rule

    def deploy_roles(self):
        return sorted({g["role"] for kind in IAM_GRANTS for g in self.values(kind) if g.get("member") == DEPLOY})

    def custom_roles(self):
        return {r["name"]: r["permissions"] for r in self.values("google_project_iam_custom_role")}

    def test_deny_policy_covers_every_sensitive_permission_deploy_holds(self):
        denied = set(self.deny_rule()["denied_permissions"])
        self.assertEqual(denied, set(SENSITIVE.values()))
        self.assertEqual(uncovered(self.deploy_roles(), self.custom_roles(), denied), [])
        # The shared-project role the Issue names is really in play.
        self.assertIn("roles/iam.serviceAccountAdmin", self.deploy_roles())

    def test_an_uncovered_allow_binding_is_detected(self):
        denied = set(self.deny_rule()["denied_permissions"])
        self.assertEqual(uncovered(["roles/iam.serviceAccountAdmin"], {}, set()), ["iam.serviceAccounts.setIamPolicy"])
        # A custom role smuggling a token permission is not covered by a policy that omits it.
        custom = {"projects/p/roles/x": ["iam.serviceAccounts.getAccessToken"]}
        self.assertEqual(uncovered(["projects/p/roles/x"], custom, denied - {SENSITIVE["iam.serviceAccounts.getAccessToken"]}), ["iam.serviceAccounts.getAccessToken"])
        with self.assertRaises(AssertionError):
            uncovered(["roles/iam.serviceAccountTokenCreator"], {}, denied)

    def test_deploy_holds_no_custom_role_with_a_sensitive_permission(self):
        for role, permissions in self.custom_roles().items():
            self.assertFalse(set(permissions) & set(SENSITIVE), role)

    def test_only_actas_is_granted_to_deploy_on_broker_accounts(self):
        brokers = {a["name"] for a in self.values("google_service_account") if a["account_id"].endswith("-broker")}
        self.assertEqual(len(brokers), 2)
        grants = [g for g in self.values("google_service_account_iam_member") if g["service_account_id"] in brokers]
        self.assertEqual(sorted(g["service_account_id"] for g in grants), sorted(brokers))
        self.assertTrue(all(g["role"] == "roles/iam.serviceAccountUser" and g["member"] == DEPLOY for g in grants))

    def test_deny_targets_only_the_deploy_identity_and_tagged_accounts(self):
        rule = self.deny_rule()
        self.assertEqual(rule["denied_principals"], ["principal://iam.googleapis.com/projects/-/serviceAccounts/aidash-deploy@aidash-fixture.iam.gserviceaccount.com"])
        self.assertFalse(rule.get("exception_principals"))
        self.assertEqual(rule["denial_condition"][0]["expression"], "resource.matchTag('aidash-fixture/aidash-credential-broker', 'broker')")
        bindings = self.values("google_tags_tag_binding")
        self.assertEqual(len(bindings), 2)

    def test_deploy_holds_no_tag_or_deny_administration(self):
        for role in self.deploy_roles():
            self.assertNotRegex(role, r"resourcemanager\.tag|iam\.deny|roles/owner|roles/editor|monitoring|logging")

    def alert_filters(self):
        return {
            policy["display_name"]: [c["condition_matched_log"][0]["filter"] for c in policy["conditions"]]
            for policy in self.values("google_monitoring_alert_policy")
        }

    def test_alerts_cover_revisions_iam_changes_and_non_broker_key_material_reads(self):
        filters = self.alert_filters()
        revision = filters["Credential Broker: new Cloud Run revision"]
        self.assertEqual(len(revision), 2)
        self.assertIn('("aidash-production-credential-broker" OR "aidash-test-credential-broker")', revision[0])
        iam = filters["Credential Broker: IAM policy changed"]
        self.assertIn("Services.SetIamPolicy", iam[0])
        self.assertIn("google.iam.admin.v1.SetIAMPolicy", iam[1])
        read = filters["Provider Credential Key Material read by a non-broker principal"]
        self.assertEqual(len(read), 1)
        self.assertIn("AccessSecretVersion", read[0])
        for email in ("aidash-production-broker", "aidash-test-broker"):
            self.assertIn(f'"{email}@aidash-fixture.iam.gserviceaccount.com"', read[0])
        self.assertEqual(read[0].count("principalEmail"), 1)

    def test_alert_policies_notify_and_rate_limit(self):
        for policy in self.values("google_monitoring_alert_policy"):
            self.assertEqual(len(policy["notification_channels"]), 1, policy["display_name"])
            self.assertTrue(policy["alert_strategy"][0]["notification_rate_limit"][0]["period"])


class DeployEntryPointTests(unittest.TestCase):
    """Every job that can obtain Google credentials runs in the protected Environment."""

    @staticmethod
    def workflow(name):
        return yaml.safe_load((WORKFLOWS / name).read_text())

    @staticmethod
    def wif_environment():
        source = (BOOTSTRAP / "variables.tf").read_text()
        return re.search(r'variable "deploy_environment" \{.*?default\s*=\s*"([^"]+)"', source, re.S).group(1)

    def credentialed_jobs(self, workflow):
        return {
            name: job for name, job in workflow["jobs"].items()
            if (job.get("permissions") or {}).get("id-token") == "write"
            or any("google-github-actions/auth" in step.get("uses", "") for step in job.get("steps", []))
        }

    def test_every_credentialed_job_declares_the_wif_environment(self):
        jobs = self.credentialed_jobs(self.workflow("gcp-environments.yml"))
        self.assertEqual(sorted(jobs), ["apply", "prepare", "publish"])
        for name, job in jobs.items():
            self.assertEqual(job.get("environment"), self.wif_environment(), name)

    def test_credentialed_jobs_request_tokens_and_uncredentialed_jobs_do_not(self):
        for name, job in self.workflow("gcp-environments.yml")["jobs"].items():
            wants_token = (job.get("permissions") or {}).get("id-token") == "write"
            self.assertEqual(wants_token, name in {"prepare", "publish", "apply"}, name)

    def test_infrastructure_checks_hold_no_cloud_credentials(self):
        workflow = self.workflow("gcp-check.yml")
        self.assertEqual(self.credentialed_jobs(workflow), {})
        self.assertNotIn("id-token", json.dumps(workflow))
        self.assertTrue(all("environment" not in job for job in workflow["jobs"].values()))

    def test_no_other_workflow_authenticates_to_google(self):
        for path in WORKFLOWS.glob("*.yml"):
            if path.name == "gcp-environments.yml":
                continue
            self.assertNotIn("google-github-actions/auth", path.read_text(), path.name)

    def test_provider_condition_names_the_environment_and_the_main_workflow(self):
        source = (BOOTSTRAP / "main.tf").read_text()
        self.assertIn("assertion.environment == '${var.deploy_environment}'", source)
        self.assertIn("assertion.ref == 'refs/heads/${var.trusted_branch}'", source)
        self.assertIn("assertion.workflow_ref == '${local.workflow_ref}'", source)


if __name__ == "__main__":
    unittest.main()
