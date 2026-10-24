"""Render the real Compose/Helm entry points and enforce deployment boundaries."""

import json
import os
from pathlib import Path
import subprocess
import tempfile
import tomllib
import unittest

import yaml


ROOT = Path(__file__).resolve().parents[1]


class StandaloneDeploymentTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        (ROOT / "private").mkdir(exist_ok=True)
        cls.temporary = tempfile.TemporaryDirectory(prefix="compose-contract-", dir=ROOT / "private")
        cls.addClassCleanup(cls.temporary.cleanup)
        cls.directory = Path(cls.temporary.name)
        cls.environment = dict(os.environ, AIDASH_STANDALONE_DIRECTORY=str(cls.directory),
                               AIDASH_AUTH_PORT="18081", AIDASH_MARKETPLACE_PORT="18082")
        cls.prepare()

    @classmethod
    def prepare(cls):
        subprocess.run(["python3", str(ROOT / "scripts/prepare-standalone-compose.py"),
                        "--directory", str(cls.directory)], env=cls.environment, check=True, capture_output=True)

    def test_preparing_again_retains_distinct_database_and_product_credentials(self):
        names = ["postgres-password", "auth-owner-password", "auth-app-password",
                 "marketplace-owner-password", "marketplace-app-password", "auth-signing-key.pem",
                 "auth-token-encryption-key", "marketplace-client-secret", "marketplace-resource-secret",
                 "marketplace-billing-secret", "marketplace-enrollment-secret", "marketplace-recall-secret"]
        before = {name: (self.directory / name).read_bytes() for name in names}
        self.assertEqual(len(set(before.values())), len(before))
        self.prepare()
        self.assertEqual({name: (self.directory / name).read_bytes() for name in names}, before)
        self.assertEqual(self.directory.stat().st_mode & 0o777, 0o700)
        subprocess.run(["python3", str(ROOT / "scripts/prepare-standalone-compose.py"), "--directory", str(self.directory),
                        "--gcip-project", "configured-project", "--gcip-tenant", "configured-pool",
                        "--gcip-api-key", "configured-public-key", "--gcip-auth-domain", "configured-project.firebaseapp.com"],
                       env=self.environment, check=True, capture_output=True)
        self.prepare()
        configured = tomllib.loads((self.directory / "auth-runtime.toml").read_text())
        self.assertEqual(configured["gcip_project"], "configured-project")
        self.assertEqual(configured["gcip_tenants"], ["configured-pool"])
        self.assertEqual(configured["gcip_browser_api_key"], "configured-public-key")

    def test_each_compose_profile_has_only_owned_migrations_and_runtime_credentials(self):
        for profile, service, prefix in (("auth", "authentication", "auth"), ("marketplace", "marketplace", "marketplace")):
            with self.subTest(profile=profile):
                model = json.loads(subprocess.check_output(
                    ["docker", "compose", "--env-file", "/dev/null", "-f", str(ROOT / "enterprise/compose.yaml"),
                     "--profile", profile, "config", "--format", "json"], env=self.environment))
                services = model["services"]
                self.assertTrue(set(services).isdisjoint({"backend", "worker", "nats", "frontend-dev"}))
                other = "marketplace" if profile == "auth" else "auth"
                self.assertNotIn(other + "-migrate", services)
                runtime = services[service]
                self.assertTrue(runtime["read_only"])
                self.assertEqual(runtime["depends_on"][prefix + "-seal"]["condition"], "service_completed_successfully")
                self.assertEqual(services[prefix + "-seal"]["depends_on"][prefix + "-migrate"]["condition"], "service_completed_successfully")
                mounted = {secret["source"] for secret in runtime["secrets"]}
                self.assertIn(prefix + "-app-url", mounted)
                self.assertFalse(any("owner" in name for name in mounted))
                self.assertNotIn("postgres-password", mounted)
                for candidate in services.values():
                    build = candidate.get("build")
                    if build:
                        self.assertTrue((Path(build["context"]) / build.get("dockerfile", "Dockerfile")).is_file())
                rendered = json.dumps(model)
                for name in ("postgres-password", "auth-owner-password", "marketplace-app-password", "marketplace-client-secret"):
                    self.assertNotIn((self.directory / name).read_text().strip(), rendered)

    def test_every_standalone_helm_example_renders_with_non_root_read_only_runtime(self):
        chart = ROOT / "enterprise/deploy/standalone-service"
        for values in sorted(chart.glob("*.values.yaml")):
            with self.subTest(values=values.name):
                output = subprocess.check_output(["helm", "template", "fixture", str(chart), "-f", str(values),
                                                  "--set-string", "image=registry.example/product@sha256:" + "a" * 64])
                objects = [item for item in yaml.safe_load_all(output) if item]
                pod = next(item for item in objects if item["kind"] == "Deployment")["spec"]["template"]["spec"]
                self.assertTrue(pod["securityContext"]["runAsNonRoot"])
                container = pod["containers"][0]
                self.assertTrue(container["securityContext"]["readOnlyRootFilesystem"])
                self.assertFalse(container["securityContext"]["allowPrivilegeEscalation"])
                self.assertEqual(container["readinessProbe"]["httpGet"]["path"], "/healthz")
                for mount in container["volumeMounts"]:
                    volume = next(volume for volume in pod["volumes"] if volume["name"] == mount["name"])
                    if "secret" in volume:
                        self.assertTrue(mount["readOnly"])


if __name__ == "__main__":
    unittest.main()
