# SPDX-FileCopyrightText: 2026 kent8192
# SPDX-License-Identifier: BUSL-1.1
# See enterprise/LICENSE.

"""Manifest registration, loopback CSRF and least-privilege credential handoff."""

import json
import os
import stat
from tempfile import TemporaryDirectory
from unittest.mock import patch
import subprocess
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "control"))
import app

APP = Path(__file__).resolve().parents[1] / "control/app.py"


class ManifestContractTests(unittest.TestCase):
    def test_review_manifest_cannot_request_webhooks_user_grants_or_extra_permissions(self):
        result = subprocess.run(
            [sys.executable, str(APP), "manifest", "--port", "8123"],
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        value = json.loads(result.stdout)
        self.assertEqual(value["default_permissions"], {
            "actions": "read", "checks": "write", "contents": "write",
            "metadata": "read", "pull_requests": "write",
        })
        self.assertEqual(value["default_events"], [])
        self.assertFalse(value["hook_attributes"]["active"])
        self.assertFalse(value["request_oauth_on_install"])
        self.assertFalse(value["public"])
        self.assertEqual(value["callback_urls"], [])


class RegistrationBoundaryTests(unittest.TestCase):
    def value(self, pem=""):
        return {"id": 1234, "client_id": "Iv23.fixture", "slug": "aidash-delivery", "pem": pem,
                "owner": {"login": "kent8192", "type": "User"}, "permissions": dict(app.PERMISSIONS)}

    def test_bad_callback_state_cannot_exchange_a_code_or_write_a_key(self):
        with TemporaryDirectory() as directory, app.RegistrationServer(directory) as server, patch.object(app.GitHubAPI, "call") as api:
            for query in ({"state": ["wrong"], "code": ["a" * 12]}, {"state": [server.state], "code": ["a" * 12], "extra": ["input"]}):
                with self.subTest(query=query), self.assertRaises(app.Refused):
                    server.callback(query)
            api.assert_not_called()
            self.assertFalse(server.finished)
            self.assertEqual(os.listdir(directory), [])

    def test_unexpected_owner_and_permission_are_refused_before_private_key_storage(self):
        for value in (dict(self.value(), owner={"login": "another-user", "type": "User"}),
                      dict(self.value(), permissions=dict(app.PERMISSIONS, administration="write"))):
            with TemporaryDirectory() as directory, self.assertRaises(app.Refused):
                app.save_registration(value, directory)
            self.assertFalse(Path(directory).exists())

    def test_registration_and_finalize_use_a_real_private_key_and_revoke_verification_token(self):
        with TemporaryDirectory() as directory:
            root = Path(directory)
            key = subprocess.check_output(["openssl", "genrsa", "2048"], stderr=subprocess.DEVNULL).decode()
            app.save_registration(self.value(key), root)
            for name in ("release-app.pem", "metadata.json"):
                self.assertEqual(stat.S_IMODE((root / name).stat().st_mode), 0o600)
            calls = []
            def api(client, method, path, value=None):
                calls.append((method, path, value, client.token))
                if path == "app":
                    return self.value()
                if path == "app/installations?per_page=100":
                    return [{"id": 5678, "account": {"login": "kent8192"}, "app_id": 1234,
                             "repository_selection": "selected", "suspended_at": None}]
                if method == "POST":
                    return {"token": "verification-token", "permissions": {"metadata": "read"}}
                if path.startswith("installation/repositories"):
                    return {"total_count": 1, "repositories": [{"full_name": "kent8192/aidash"}]}
                return None
            with patch.object(app.GitHubAPI, "call", api), patch.object(app, "install_secrets", return_value={}) as install:
                result = app.finalize(root)
            self.assertTrue(result["protected_key_installed"])
            self.assertEqual(install.call_args.args[0], {"release-automation": {"RELEASE_APP_PRIVATE_KEY": key}})
            self.assertIn(("DELETE", "installation/token", None, "verification-token"), calls)
            public = json.loads((root / "release_app.tfvars.json").read_text())
            self.assertEqual(public["release_app"]["installation_id"], 5678)
            self.assertNotIn(key, json.dumps(public))
            with self.assertRaises(app.Refused):
                app.save_registration(self.value(key), root)

    def test_all_repository_installation_and_overbroad_verification_token_are_refused(self):
        metadata = app.validate_app(self.value())
        for case in ("all", "repositories", "permissions"):
            calls = []
            def api(client, method, path, value=None, case=case, calls=calls):
                calls.append((method, path))
                if path == "app":
                    return self.value()
                if path.startswith("app/installations?"):
                    return [{"id": 5678, "account": {"login": "kent8192"}, "app_id": 1234,
                             "repository_selection": "all" if case == "all" else "selected", "suspended_at": None}]
                if method == "POST":
                    return {"token": "verification-token", "permissions": {"metadata": "read", "contents": "write"} if case == "permissions" else {"metadata": "read"}}
                if method == "GET":
                    return {"total_count": 2, "repositories": [{"full_name": "kent8192/aidash"}, {"full_name": "kent8192/another"}]}
                return None
            with self.subTest(case=case), patch.object(app.GitHubAPI, "call", api), self.assertRaises(app.Refused):
                app.validate_installations(metadata, app.GitHubAPI("jwt"))
            self.assertEqual(("DELETE", "installation/token") in calls, case != "all")


if __name__ == "__main__":
    unittest.main()
