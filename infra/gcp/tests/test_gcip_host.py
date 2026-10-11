"""GCIP Terraform outputs reach the host's actual Reinhardt settings input."""

from contextlib import ExitStack
from copy import deepcopy
import json
from pathlib import Path
import sys
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import patch
from urllib.error import HTTPError

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "control"))
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "runtime"))
import controller
import host
from test_gcip import OUTPUT


class GcipHostTests(unittest.TestCase):
    def setUp(self):
        self.context = ExitStack()
        self.addCleanup(self.context.close)
        root = Path(self.context.enter_context(TemporaryDirectory()))
        self.context.enter_context(patch.object(host, "ROOT", root / "data"))
        self.context.enter_context(patch.object(host, "RUN", root / "run"))
        self.context.enter_context(patch.object(host, "cloud_token", return_value="fixture"))
        self.environment = {"project": "aidash-fixture", "hostname": "test.aidash.run", "secret": "runtime"}

    def configure(self, value):
        payload = {"payload": {"data": host.base64.b64encode(json.dumps(value).encode()).decode()}}
        def request(url, *args, **kwargs):
            # A host without BYOK has no Provider Credential metadata attribute.
            if url.endswith("/instance/attributes/aidash-provider-credentials"):
                raise HTTPError(url, 404, "absent", {}, None)
            return json.dumps(payload).encode()

        with patch.object(host, "request", side_effect=request):
            host.configuration(self.environment)
        return dict(line.split("=", 1) for line in (host.RUN / "app.env").read_text().splitlines())

    def test_controller_bindings_reach_gcip_settings_without_legacy_issuer(self):
        raw = {"AIDASH_SECRET_FIXTURE": "private", "dashboard": {"gcip": {"web_api_key": "public"}}}

        def command(*args, **kwargs):
            if "list" in args:
                return b'[{"name":"projects/aidash-fixture/secrets/runtime/versions/1","state":"ENABLED"}]'
            if "access" in args:
                return json.dumps(raw).encode()
            if "add" in args:
                raw.clear()
                raw.update(json.loads(kwargs["data"]))
            return b""

        with patch.object(controller, "run", side_effect=command):
            controller.provision_secret({"project_id": "aidash-fixture"}, OUTPUT, "test")
            values = self.configure(raw)
            path = Path(values["AIDASH_GCIP_SETTINGS"])
            self.assertEqual(json.loads(path.read_text()), {"dashboard": raw["dashboard"]})
            self.assertEqual(path.stat().st_mode & 0o777, 0o644)
            self.assertEqual(path.parent.stat().st_mode & 0o777, 0o755)
            self.assertFalse(any(key.startswith("AIDASH_OIDC_") for key in values))
            self.assertEqual(values["AIDASH_SECRET_FIXTURE"], "private")
            self.assertNotIn("private", path.read_text())
            self.assertEqual(values["AIDASH_NODE_ID"], "aidash://runtime")
            self.assertEqual(json.loads(path.read_text())["dashboard"]["gcip"]["auth_helper"], "public_origin")

            removed = deepcopy(OUTPUT)
            removed["gcip"]["tenant_ids"] = []
            controller.provision_secret({"project_id": "aidash-fixture"}, removed, "test")
            self.configure(raw)
            settings = json.loads(path.read_text())["dashboard"]["gcip"]
            self.assertEqual(settings["tenant_bindings"], {})
            self.assertEqual(settings["providers"], {})
            self.assertEqual(settings["password_sign_up"], [])

    def test_no_gcip_keeps_existing_google_oidc_path(self):
        values = self.configure({"AIDASH_OIDC_CLIENT_ID": "client", "AIDASH_OIDC_CLIENT_SECRET": "secret"})
        self.assertEqual(values["AIDASH_OIDC_ISSUER"], "https://accounts.google.com")
        self.assertEqual(values["AIDASH_OIDC_PUBLIC_ORIGIN"], "https://test.aidash.run")
        self.assertNotIn("AIDASH_GCIP_SETTINGS", values)

    def test_mixed_issuer_and_other_settings_are_refused(self):
        settings = {key: OUTPUT["gcip"][key] for key in ("project_id", "public_origin", "tenant_bindings", "providers", "password_sign_up")}
        settings["web_api_key"] = "public"
        for value in [
            {"dashboard": {"gcip": settings}, "AIDASH_OIDC_CLIENT_ID": "legacy"},
            {"dashboard": {"gcip": settings, "oidc": {}}},
            {"dashboard": {"gcip": dict(settings, project_id="another-project")}},
            {"dashboard": {"gcip": dict(settings, public_origin="https://other.invalid")}},
            {"dashboard": {"gcip": dict(settings, arbitrary="value")}},
            {"dashboard": {"gcip": dict(settings, auth_helper="auth.example.test")}},
            {"dashboard": {"gcip": settings}, "AIDASH_GCIP_SETTINGS": "/override"},
        ]:
            with self.subTest(value=value), self.assertRaises(ValueError):
                self.configure(value)
        self.assertFalse((host.RUN / "app.env").exists())

    def test_only_a_public_origin_helper_is_proxied_ahead_of_admission(self):
        environment = dict(self.environment, project="aidash-fixture")
        for gcip in [None, {}, {"auth_helper": "firebase"}]:
            with self.subTest(gcip=gcip):
                rendered = host.caddyfile(environment, gcip)
                self.assertNotIn("/__/", rendered)
                self.assertIn("reverse_proxy 127.0.0.1:8088", rendered)
        rendered = host.caddyfile(environment, {"auth_helper": "public_origin"})
        helper, application = rendered.split("  handle {\n")
        self.assertIn("@firebase_helper path /__/auth/* /__/firebase/init.json", helper)
        self.assertIn("handle @firebase_helper {", helper)
        self.assertIn("reverse_proxy https://aidash-fixture.firebaseapp.com {", helper)
        # Aidash session/CSRF cookies must never be forwarded to Google.
        for directive in ["header_up -Cookie", "header_up -Authorization", "header_down -Set-Cookie"]:
            self.assertIn(directive, helper)
        self.assertIn("reverse_proxy 127.0.0.1:8088", application)
