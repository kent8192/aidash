"""Provider Credential IAM boundaries; workloads receive no runtime-config secret grant."""
import re
import unittest
from pathlib import Path
ROOT = Path(__file__).resolve().parents[1]

def resource(source, kind, name):
    start = re.search(rf'resource "{kind}" "{name}"\s*\{{', source)
    if not start:
        raise AssertionError(f"Missing {kind}.{name}")
    # Ignore quoted strings (including Terraform interpolation) when balancing HCL.
    depth = 1
    rest = source[start.end():]
    for token in re.finditer(r'"(?:\\.|[^"\\])*"|[{}]', rest):
        if token.group() == "{":
            depth += 1
        elif token.group() == "}":
            depth -= 1
        if depth == 0:
            return rest[:token.start()]
    raise AssertionError("Unbalanced HCL resource")

class ProviderCredentialIamTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.byok = (ROOT / "modules/environment/provider_credentials.tf").read_text()
        cls.shared = (ROOT / "modules/environment/main.tf").read_text()
        cls.bootstrap = (ROOT / "bootstrap/provider_credentials.tf").read_text()

    def test_runtime_has_only_create_and_prefix_manage_in_byok(self):
        self.assertNotIn('resource "google_project_iam_custom_role"', self.byok)
        create = resource(self.bootstrap, "google_project_iam_custom_role", "byok_create")
        manage = resource(self.bootstrap, "google_project_iam_custom_role", "byok_manage")
        self.assertEqual(set(re.findall(r'"(secretmanager\.[^"]+)"', create)), {"secretmanager.secrets.create"})
        self.assertEqual(set(re.findall(r'"(secretmanager\.[^"]+)"', manage)), {
            "secretmanager.secrets.get", "secretmanager.secrets.delete",
            "secretmanager.versions.add", "secretmanager.versions.disable",
            "secretmanager.versions.destroy", "secretmanager.versions.get", "secretmanager.versions.list",
        })
        bindings = re.findall(r'resource "google_project_iam_member" "([^"]+)"', self.byok)
        self.assertEqual(set(bindings), {"provider_credential_create", "provider_credential_manage"})
        for name in bindings:
            body = resource(self.byok, "google_project_iam_member", name)
            self.assertRegex(body, r'project\s*=\s*var\.byok_project_id')
            self.assertIn('google_service_account.workload["server"].email', body)
            self.assertNotIn('"worker"', body)
            self.assertNotIn('secretAccessor', body)
        create_binding = resource(self.byok, "google_project_iam_member", "provider_credential_create")
        self.assertIn('"projects/${var.byok_project_id}/roles/aidashByokCreate"', create_binding)
        self.assertNotIn("condition {", create_binding)
        manage_binding = resource(self.byok, "google_project_iam_member", "provider_credential_manage")
        self.assertIn('"projects/${var.byok_project_id}/roles/aidashByokManage"', manage_binding)
        self.assertIn("data.google_project.byok[0].number", manage_binding)
        self.assertIn("local.provider_credential_secret_prefix", manage_binding)
        self.assertIn("resource.name.startsWith", manage_binding)
        self.assertNotIn("secretmanager.versions.access", self.byok)
        self.assertNotIn("setIamPolicy", self.byok)
        self.assertNotIn('google_secret_manager_secret_iam_member', self.byok)
        self.assertIn('"aidash-${var.environment_id}-cred-"', self.byok)
        self.assertNotIn('incarnation', self.byok)

    def test_deploy_can_modify_only_fixed_write_roles(self):
        deploy = resource(self.bootstrap, "google_project_iam_custom_role", "byok_deploy")
        self.assertEqual(set(re.findall(r'"((?:iam|resourcemanager|secretmanager)\.[^"]+)"', deploy)), {
            "resourcemanager.projects.get", "resourcemanager.projects.getIamPolicy",
            "resourcemanager.projects.setIamPolicy",
        })
        binding = resource(self.bootstrap, "google_project_iam_member", "byok_deploy")
        self.assertRegex(binding, r'project\s*=\s*var\.byok_project_id')
        self.assertIn('google_service_account.automation["deploy"].email', binding)
        self.assertRegex(binding, r'role\s*=\s*google_project_iam_custom_role\.byok_deploy\[0\]\.name')
        expression = re.search(r'expression\s*=\s*"([^"]+)"', binding).group(1)
        self.assertEqual(expression,
            "api.getAttribute('iam.googleapis.com/modifiedGrantsByRole', []).hasOnly("
            "['projects/${var.byok_project_id}/roles/aidashByokCreate', "
            "'projects/${var.byok_project_id}/roles/aidashByokManage'])")

    def test_retirement_is_bootstrap_only_delete_and_project_inventory(self):
        for name, permission in [("byok_retire", "secretmanager.secrets.delete"),
                                 ("byok_retire_inventory", "secretmanager.secrets.list")]:
            role = resource(self.bootstrap, "google_project_iam_custom_role", name)
            self.assertEqual(set(re.findall(r'"((?:iam|resourcemanager|secretmanager)\.[^"]+)"', role)), {permission})
            self.assertNotIn("versions.", role)
            grant = resource(self.bootstrap, "google_project_iam_member", name)
            self.assertIn('google_service_account.automation["deploy"].email', grant)
            self.assertRegex(grant, r'project\s*=\s*var\.byok_project_id')
            self.assertIn(f'google_project_iam_custom_role.{name}[0].name', grant)
            self.assertNotIn(name, self.byok)
        deletion = resource(self.bootstrap, "google_project_iam_member", "byok_retire")
        self.assertIn("resource.name.startsWith('projects/${data.google_project.byok[0].number}/secrets/aidash-')", deletion)
        inventory = resource(self.bootstrap, "google_project_iam_member", "byok_retire_inventory")
        self.assertNotIn("condition {", inventory)

    def test_only_bootstrap_broker_has_byok_payload_read_grant(self):
        role = resource(self.bootstrap, "google_project_iam_custom_role", "byok_broker_read")
        self.assertIn('"aidashByokBrokerRead"', role)
        self.assertEqual(set(re.findall(r'"(secretmanager\.[^"]+)"', role)), {
            "secretmanager.versions.access", "secretmanager.versions.get", "secretmanager.secrets.get",
        })
        # Inventory every source, including secret-level IAM: bootstrap is the
        # only permitted binding of the fixed payload-read role.
        readers = []
        byok_grants = []
        for root in [ROOT / "bootstrap", ROOT / "environments", ROOT / "modules/environment", ROOT / "modules/credential-broker"]:
            for path in root.glob("*.tf"):
                source = path.read_text()
                for kind, name in re.findall(r'resource "([^"]+)" "([^"]+)"', source):
                    body = resource(source, kind, name)
                    if kind == "google_project_iam_custom_role" and '"secretmanager.versions.access"' in body:
                        readers.append((path.relative_to(ROOT).as_posix(), name))
                    if kind.endswith(("_iam_member", "_iam_binding", "_iam_policy")):
                        if "byok_broker_read" in body or "aidashByokBrokerRead" in body:
                            self.assertEqual((path.relative_to(ROOT).as_posix(), name), ("bootstrap/credential_brokers.tf", "byok_broker_read"))
                            self.assertIn("google_service_account.broker[each.key].email", body)
                            self.assertIn("data.google_project.byok[0].number", body)
                            self.assertIn("aidash-${each.key}-cred-", body)
                        if "var.byok_project_id" in body:
                            self.assertNotIn('roles/secretmanager.', body)
                            byok_grants.append((path.relative_to(ROOT).as_posix(), name))
                        self.assertFalse(kind.startswith("google_secret_manager_secret_iam_"),
                                         (path.relative_to(ROOT).as_posix(), name))
        self.assertEqual(readers, [("bootstrap/provider_credentials.tf", "byok_broker_read")])
        self.assertEqual(set(byok_grants), {
            ("bootstrap/provider_credentials.tf", "byok_deploy"),
            ("bootstrap/credential_brokers.tf", "byok_broker_read"),
            ("bootstrap/provider_credentials.tf", "byok_retire"),
            ("bootstrap/provider_credentials.tf", "byok_retire_inventory"),
            ("modules/environment/provider_credentials.tf", "provider_credential_create"),
            ("modules/environment/provider_credentials.tf", "provider_credential_manage"),
        })

    def test_workloads_have_no_shared_project_secret_access(self):
        self.assertIn('resource "google_secret_manager_secret" "runtime"', self.shared)
        for source in (self.shared, self.byok):
            for name in re.findall(r'resource "google_project_iam_member" "([^"]+)"', source):
                binding = resource(source, "google_project_iam_member", name)
                self.assertNotIn('roles/secretmanager.', binding)
        self.assertNotIn('secretmanager.secrets.create', self.shared)

    def test_audit_and_bootstrap_are_on_explicit_byok_project(self):
        audit = resource(self.bootstrap, "google_project_iam_audit_config", "byok_secret_manager")
        self.assertRegex(audit, r'project\s*=\s*var\.byok_project_id')
        self.assertIn('"secretmanager.googleapis.com"', audit)
        self.assertIn('"DATA_READ"', audit)
        self.assertIn('"DATA_WRITE"', audit)
        service = resource(self.bootstrap, "google_project_service", "byok_secret_manager")
        self.assertRegex(service, r'project\s*=\s*var\.byok_project_id')
        self.assertNotIn('resource "google_project"', self.bootstrap)
        deploy = resource(self.bootstrap, "google_project_iam_custom_role", "byok_deploy")
        self.assertNotIn('secretmanager.', deploy)
        self.assertNotIn('roles/owner', self.bootstrap)
        for name in ['byok_project_id', 'secret_prefix']:
            self.assertIn(f'output "{name}"', self.byok)
