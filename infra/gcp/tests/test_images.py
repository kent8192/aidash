"""Source capability guards must accept the native Cargo workspace layout."""

from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import Mock, patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "control"))
import images


class AppImageTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.source = Path(directory.name) / "source"
        self.output = Path(directory.name) / "archives"
        backend = self.source / "server/src"
        backend.mkdir(parents=True)
        (backend / "config.rs").write_text('const GOOGLE_OIDC_ISSUER: &str = "fixture";\n')
        (backend / "http.rs").write_text('const SETTING: &str = "AIDASH_AUTH_TRUSTED_PROXY_IPS";\n')
        self.sha = "a" * 40
        self.docker = Mock()

    def build(self, kind="app"):
        arguments = ["images.py", "build", "--kind", kind, "--source", str(self.source),
                     "--sha", self.sha, "--directory", str(self.output)]
        with patch.object(sys, "argv", arguments), \
             patch.object(images.subprocess, "check_output", return_value=self.sha + "\n"), \
             patch.object(images.subprocess, "run", self.docker):
            images.main()
        return self.docker

    def test_workspace_source_reaches_build_and_archive_without_old_paths(self):
        # Arrange: no legacy src directory exists in this checkout.
        self.assertFalse((self.source / "src").exists())
        # Act
        docker = self.build()
        # Assert: source guards passed; deployment artifacts use the authorized SHA.
        self.assertEqual(docker.call_count, 2)
        command = docker.call_args_list[0].args[0]
        self.assertEqual(command[:2], ["docker", "build"])
        self.assertIn("--target", command)
        self.assertEqual(command[-1], str(self.source))
        self.assertEqual(docker.call_args_list[1].args[0], [
            "docker", "save", "--output", str(self.output / "app.tar"), f"aidash-app:{self.sha}",
        ])

    def test_missing_login_support_is_rejected_before_build(self):
        (self.source / "server/src/config.rs").write_text("// Unsupported source\n")
        with self.assertRaisesRegex(SystemExit, "required Google login integration"):
            self.build()
        self.docker.assert_not_called()

    def test_missing_proxy_support_is_rejected_before_build(self):
        (self.source / "server/src/http.rs").write_text("// Unsupported source\n")
        with self.assertRaisesRegex(SystemExit, "trusted-proxy authentication rate limits"):
            self.build()
        self.docker.assert_not_called()

    def test_privileged_kinds_build_only_from_the_trusted_checkout(self):
        for kind, dockerfile in [("control", "runner/control.Dockerfile"),
                                 ("edge", "infra/gcp/helm/environment/edge.Dockerfile"),
                                 ("observer", "infra/gcp/observer/Dockerfile")]:
            with self.subTest(kind=kind):
                self.docker.reset_mock()
                command = self.build(kind).call_args_list[0].args[0]
                self.assertEqual(command[command.index("--file") + 1],
                                 str(images.ROOT / dockerfile))
                self.assertTrue(Path(command[-1]).is_relative_to(images.ROOT))
                self.assertFalse(Path(command[-1]).is_relative_to(self.source))

    def test_sandbox_context_is_the_deployed_source(self):
        command = self.build("sandbox").call_args_list[0].args[0]
        self.assertEqual(command[-1], str(self.source / "runner"))

    def test_caddy_is_copied_from_its_reviewed_digest(self):
        docker = self.build("caddy")
        self.assertEqual(docker.call_args_list[0].args[0], [
            "docker", "pull", "--platform", "linux/amd64",
            "caddy:2.10.2@sha256:c3d7ee5d2b11f9dc54f947f68a734c84e9c9666c92c88a7f30b9cba5da182adb",
        ])
        self.assertEqual(docker.call_args_list[1].args[0][-1], f"aidash-caddy:{self.sha}")


if __name__ == "__main__":
    unittest.main()
