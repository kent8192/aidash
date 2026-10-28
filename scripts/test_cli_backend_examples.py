"""Repository contracts that do not need credentials or a running model."""
import json
from pathlib import Path
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[1]


class CliBackendExamples(unittest.TestCase):
    def test_examples_are_trackable_and_private_envs_are_ignored(self):
        for profile in ("claude", "codex-native", "codex-access-token", "codex-api-key"):
            example = f"examples/cli-backends/{profile}/.env.example"
            private = f"private/cli-backends/{profile}/.env"
            self.assertTrue((ROOT / example).is_file())
            for path, expected in ((example, 1), (private, 0)):
                result = subprocess.run(["git", "check-ignore", "--no-index", "-q", path], cwd=ROOT, check=False)
                self.assertEqual(result.returncode, expected, path)

    def test_credentials_do_not_enter_server_env_or_registry(self):
        for path in [ROOT / ".env.example", *sorted((ROOT / "examples/cli-backends/registry").glob("*.json"))]:
            text = path.read_text()
            for secret in ("CODEX_ACCESS_TOKEN=", "CODEX_API_KEY=", '"credential_env"', '"plan_connection"', '"provider_credential"'):
                self.assertNotIn(secret, text)

    def test_exact_origins_and_endpoint_paths(self):
        for provider, port in (("claude", 18190), ("codex", 18191)):
            model = json.loads((ROOT / f"examples/cli-backends/registry/model-{provider}.example.json").read_text())
            self.assertEqual(model["config"]["endpoint"], f"http://127.0.0.1:{port}/v1")
            self.assertFalse(model["config"]["streaming"])
        settings = (ROOT / "examples/cli-backends/settings/keyless-endpoints.example.toml").read_text()
        self.assertIn('"http://127.0.0.1:18190"', settings)
        self.assertNotIn('/v1"', settings)


if __name__ == "__main__":
    unittest.main()
