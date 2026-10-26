"""Exercise the Community OpenAPI denylist against inline descriptions."""

import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


SCRIPT = Path(__file__).with_name("check-community-openapi.py")
SPEC = importlib.util.spec_from_file_location("community_openapi", SCRIPT)
OPENAPI = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(OPENAPI)

ACCEPTED = {
    "openapi": "3.1.0",
    "paths": {
        "/api/runs": {"get": {"tags": ["Runs"]}},
        "/api/marketplace/installations": {"post": {"tags": ["Marketplace"]}},
    },
    "tags": [{"name": "Runs"}, {"name": "Marketplace"}],
}

REJECTED = {
    "openapi": "3.1.0",
    "paths": {
        "/api/gcip/tenants": {"get": {"tags": ["Runs"]}},
        "/api/runs": {"get": {"tags": ["Credential broker"]}},
        "/api/billing/invoices": {"get": {"tags": []}},
        "/api/secret-manager/secrets": {"get": {"tags": []}},
        "/api/kms/keys": {"get": {"tags": []}},
    },
    "tags": [{"name": "Billing"}, {"name": "Runs"}],
}


def write(document: dict) -> tuple[tempfile.TemporaryDirectory, Path]:
    directory = tempfile.TemporaryDirectory()
    path = Path(directory.name) / "aidash.json"
    path.write_text(json.dumps(document))
    return directory, path


class CommunityOpenAPITests(unittest.TestCase):
    def test_accepts_a_description_without_cloud_only_names(self):
        self.assertEqual(OPENAPI.violations(ACCEPTED), [])

    def test_rejects_cloud_only_paths(self):
        problems = OPENAPI.violations(REJECTED)
        self.assertIn("Cloud-only path: /api/gcip/tenants", problems)
        self.assertIn("Cloud-only path: /api/billing/invoices", problems)
        self.assertIn("Cloud-only path: /api/secret-manager/secrets", problems)
        self.assertIn("Cloud-only path: /api/kms/keys", problems)

    def test_rejects_cloud_only_tags_on_operations_and_the_description(self):
        problems = OPENAPI.violations(REJECTED)
        self.assertIn("Cloud-only tag: Credential broker", problems)
        self.assertIn("Cloud-only tag: Billing", problems)

    def test_matches_every_denylisted_name(self):
        for name in ("gcip", "broker", "billing", "secret-manager", "secret_manager", "kms"):
            with self.subTest(name=name):
                document = {"paths": {f"/api/{name}/items": {"get": {"tags": ["Runs"]}}}}
                self.assertEqual(len(OPENAPI.violations(document)), 1)

    def test_command_exit_codes(self):
        def run(document: dict) -> int:
            directory, path = write(document)
            try:
                return subprocess.run(
                    [sys.executable, str(SCRIPT), str(path)], capture_output=True
                ).returncode
            finally:
                directory.cleanup()

        self.assertEqual(run(ACCEPTED), 0)
        self.assertEqual(run(REJECTED), 1)
        self.assertEqual(subprocess.run([sys.executable, str(SCRIPT)], capture_output=True).returncode, 2)

    def test_refuses_an_unreadable_description(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "aidash.json"
            path.write_text("not json")
            result = subprocess.run([sys.executable, str(SCRIPT), str(path)], capture_output=True)
        self.assertEqual(result.returncode, 2)
        self.assertIn(b"unreadable OpenAPI description", result.stderr)


if __name__ == "__main__":
    unittest.main()
