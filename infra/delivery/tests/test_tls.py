"""Wildcard renewal contract: actual certificate validation and private transport."""

from contextlib import redirect_stdout
from copy import deepcopy
import io
import json
import os
from pathlib import Path
import stat
import subprocess
import sys
from tempfile import TemporaryDirectory
import unittest
import yaml
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "control"))
import tls
from policy import Refused

CONFIG = {
    "project_id": "aidash-fixture", "deployment_tier": "nonproduction", "domain": "aidash.run",
    "preview_tls_secret": "aidash-preview-wildcard-tls", "repository": "kent8192/aidash", "automation_branch": "develop/0.1.0",
}
TOKEN = "private-cloudflare-token-" + "a" * 20
ACCOUNT = {"schema": "aidash-acme-account/1", "files": {
    "accounts/acme-v02.api.letsencrypt.org/directory/" + "c" * 32 + "/" + name: json.dumps(value)
    for name, value in [("private_key.json", {"kty": "RSA", "d": "PRIVATE_ACCOUNT_KEY"}),
                        ("regr.json", {"uri": "https://acme-v02.api.letsencrypt.org/acme/acct/123"}),
                        ("meta.json", {"creation_host": "fixture"})]
}}


class WorkflowContractTests(unittest.TestCase):
    def test_renewal_reads_the_same_public_keys_managed_by_terraform(self):
        workflow = yaml.safe_load((Path(__file__).resolve().parents[3] / ".github/workflows/deployment-certificate.yml").read_text())
        self.assertIn("vars.GCP_CERTIFICATE_RENEWAL_ENABLED == 'true'", workflow["jobs"]["intake"]["if"])
        renew = workflow["jobs"]["renew"]
        self.assertEqual(renew["environment"], "certificate-renewal")
        step = next(item for item in renew["steps"] if item.get("name") == "Renew only the managed preview wildcard")
        self.assertEqual(step["env"]["AIDASH_ACME_EMAIL"], "${{ vars.AIDASH_ACME_EMAIL }}")
        self.assertEqual(step["env"]["CLOUDFLARE_CERTIFICATE_DNS_TOKEN"], "${{ secrets.CLOUDFLARE_CERTIFICATE_DNS_TOKEN }}")


class SecretManager:
    def __init__(self, bundle=None, revoked_latest=False):
        self.bundle = bundle
        self.revoked_latest = revoked_latest
        self.commands = []
        self.payloads = []
        self.account_state = None
        self.account_payloads = []

    def run(self, *args, **kwargs):
        if args[0] != "gcloud":
            return original_run(*args, **kwargs)
        self.commands.append(args)
        secret = args[args.index("--secret") + 1] if "--secret" in args else args[4]
        bundle = self.bundle if secret == CONFIG["preview_tls_secret"] else self.account_state
        if "list" in args:
            # `gcloud secrets versions list` requires a positional SECRET and has no --secret flag.
            if args[1:4] != ("secrets", "versions", "list") or "--secret" in args or args[4].startswith("-"):
                raise AssertionError("gcloud secrets versions list requires the positional SECRET")
            versions = [] if bundle is None else [{"name": f"projects/123/secrets/{secret}/versions/1", "state": "ENABLED"}]
            if self.revoked_latest and secret == CONFIG["preview_tls_secret"]:
                versions.append({"name": "projects/123/secrets/aidash-preview-wildcard-tls/versions/2", "state": "DISABLED"})
            return json.dumps(versions).encode()
        if "access" in args:
            return json.dumps(bundle).encode()
        if "add" in args:
            if secret != CONFIG["preview_tls_secret"]:
                self.account_payloads.append(kwargs["data"])
                self.account_state = json.loads(kwargs["data"])
                return b""
            self.payloads.append(kwargs["data"])
            self.bundle = json.loads(kwargs["data"])
            self.revoked_latest = False
            return b""
        raise AssertionError("unexpected secret operation")


original_run = tls.run


class CertificateTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.files = TemporaryDirectory()
        directory = Path(cls.files.name)
        cls.bundles = {}
        for name, host, days in [("healthy", tls.DOMAIN, 90), ("due", tls.DOMAIN, 10), ("wrong", "*.aidash.run", 90)]:
            cert, key = directory / (name + ".pem"), directory / (name + ".key")
            subprocess.run(["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", str(days),
                            "-subj", "/CN=" + host, "-addext", "subjectAltName=DNS:" + host,
                            "-out", str(cert), "-keyout", str(key)],
                           check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            cls.bundles[name] = {"certificate_pem": cert.read_text(), "private_key_pem": key.read_text()}

    @classmethod
    def tearDownClass(cls):
        cls.files.cleanup()

    def test_fresh_secret_is_issued_once_and_reused_without_another_version(self):
        cloud = SecretManager()
        with patch("tls.run", side_effect=cloud.run), patch("tls.issue_certificate", return_value=self.bundles["healthy"]) as issue:
            self.assertEqual(tls.renew(CONFIG, "maintainer@example.test", TOKEN), "renewed")
            self.assertEqual(tls.renew(CONFIG, "maintainer@example.test", TOKEN), "current")
        self.assertEqual(issue.call_count, 1)
        self.assertEqual([json.loads(value) for value in cloud.payloads], [self.bundles["healthy"]])
        argv = json.dumps(cloud.commands)
        self.assertNotIn(TOKEN, argv)
        self.assertNotIn(self.bundles["healthy"]["private_key_pem"], argv)
        self.assertTrue(all("--project" in command and CONFIG["project_id"] in command for command in cloud.commands))
        self.assertTrue(any("--data-file=-" in command for command in cloud.commands))

    def test_due_or_revoked_latest_is_replaced_and_invalid_bundle_is_never_published(self):
        for previous, revoked in [(self.bundles["due"], False), (self.bundles["healthy"], True)]:
            with self.subTest(revoked=revoked):
                cloud = SecretManager(previous, revoked)
                with patch("tls.run", side_effect=cloud.run), patch("tls.issue_certificate", return_value=self.bundles["healthy"]):
                    self.assertEqual(tls.renew(CONFIG, "maintainer@example.test", TOKEN), "renewed")
                self.assertEqual(len(cloud.payloads), 1)
        for invalid in [self.bundles["wrong"], dict(self.bundles["healthy"], private_key_pem=self.bundles["wrong"]["private_key_pem"]), self.bundles["due"]]:
            with self.subTest(invalid=invalid["certificate_pem"][:20]):
                cloud = SecretManager()
                with patch("tls.run", side_effect=cloud.run), patch("tls.issue_certificate", return_value=invalid), self.assertRaises(Refused):
                    tls.renew(CONFIG, "maintainer@example.test", TOKEN)
                self.assertEqual(cloud.payloads, [])

    def test_wrong_existing_domain_and_untrusted_target_fail_before_issuance(self):
        cloud = SecretManager(self.bundles["wrong"])
        with patch("tls.run", side_effect=cloud.run), patch("tls.issue_certificate") as issue:
            with self.assertRaisesRegex(Refused, "domain"):
                tls.renew(CONFIG, "maintainer@example.test", TOKEN)
            for changes in [{"domain": "other.example"}, {"deployment_tier": "production"}, {"preview_tls_secret": "../runtime"}]:
                with self.subTest(changes=changes), self.assertRaises(Refused):
                    tls.renew(dict(CONFIG, **changes), "maintainer@example.test", TOKEN)
        issue.assert_not_called()
        self.assertEqual(cloud.payloads, [])

    def test_concurrent_valid_publisher_supersedes_our_issuance(self):
        cloud = SecretManager(self.bundles["due"])

        def issue(*args):
            cloud.bundle = deepcopy(self.bundles["healthy"])
            return self.bundles["healthy"]

        with patch("tls.run", side_effect=cloud.run), patch("tls.issue_certificate", side_effect=issue):
            self.assertEqual(tls.renew(CONFIG, "maintainer@example.test", TOKEN), "superseded")
        self.assertEqual(cloud.payloads, [])

    def test_certbot_receives_only_private_ephemeral_dns_credentials_and_pinned_image(self):
        observed = {}

        def docker(*args, **kwargs):
            self.assertEqual(args[:3], ("docker", "run", "--rm"))
            self.assertIn(tls.CERTBOT, args)
            self.assertRegex(tls.CERTBOT, r"^certbot/dns-cloudflare:v[0-9.]+@sha256:[a-f0-9]{64}$")
            self.assertNotIn(TOKEN, json.dumps(args))
            self.assertIn("--read-only", args)
            self.assertIn("no-new-privileges", args)
            mount = args[args.index("--mount") + 1]
            directory = Path(mount.split(",")[1].removeprefix("source="))
            credentials = directory / "cloudflare.ini"
            observed["directory"] = directory
            self.assertEqual(stat.S_IMODE(directory.stat().st_mode), 0o700)
            self.assertEqual(stat.S_IMODE(credentials.stat().st_mode), 0o600)
            self.assertEqual(credentials.read_text(), "dns_cloudflare_api_token = " + TOKEN + "\n")
            for name, value in ACCOUNT["files"].items():
                path = directory / "config" / name
                path.parent.mkdir(parents=True, exist_ok=True)
                tls.private_file(path, value)
            output = directory / "config" / "live" / "aidash-preview"
            output.mkdir(parents=True)
            for name, value in [("fullchain.pem", self.bundles["healthy"]["certificate_pem"]), ("privkey.pem", self.bundles["healthy"]["private_key_pem"])]:
                tls.private_file(output / name, value)
            return b"private certificate log which must be ignored"

        states = []
        with patch("tls.run", side_effect=docker):
            self.assertEqual(tls.issue_certificate("maintainer@example.test", TOKEN, None, states.append), self.bundles["healthy"])
        self.assertEqual(states, [ACCOUNT])
        self.assertFalse(observed["directory"].exists())

    def test_failed_dns_challenge_persists_account_and_retry_restores_it_without_secret_logs(self):
        cloud = SecretManager()
        attempts = []
        stopped = []

        def command(*args, **kwargs):
            if args[0] != "docker":
                return cloud.run(*args, **kwargs)
            if args[1] == "rm":
                stopped.append(args[-1])
                return b""
            root = Path(args[args.index("--mount") + 1].split(",")[1].removeprefix("source="))
            attempts.append(root)
            tls.private_file(root / "container.cid", "f" * 64)
            if len(attempts) == 1:
                for name, value in ACCOUNT["files"].items():
                    path = root / "config" / name
                    path.parent.mkdir(parents=True, exist_ok=True)
                    tls.private_file(path, value)
                raise RuntimeError("DNS rejected " + TOKEN)
            for name, value in ACCOUNT["files"].items():
                path = root / "config" / name
                self.assertEqual(path.read_text(), value)
                self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o600)
            live = root / "config" / "live" / "aidash-preview"
            live.mkdir(parents=True)
            tls.private_file(live / "fullchain.pem", self.bundles["healthy"]["certificate_pem"])
            tls.private_file(live / "privkey.pem", self.bundles["healthy"]["private_key_pem"])
            return b""

        with patch("tls.run", side_effect=command):
            with self.assertRaises(RuntimeError):
                tls.renew(CONFIG, "maintainer@example.test", TOKEN)
            self.assertEqual(cloud.payloads, [])
            self.assertEqual(cloud.account_state, ACCOUNT)
            self.assertEqual(tls.renew(CONFIG, "maintainer@example.test", TOKEN), "renewed")
        self.assertEqual(len(cloud.account_payloads), 1)
        self.assertEqual(stopped, ["f" * 64, "f" * 64])
        self.assertTrue(all(not path.exists() for path in attempts))
        self.assertNotIn("PRIVATE_ACCOUNT_KEY", json.dumps(cloud.commands))
        wrong_endpoint = deepcopy(ACCOUNT)
        registration = next(name for name in wrong_endpoint["files"] if name.endswith("regr.json"))
        wrong_endpoint["files"][registration] = json.dumps({"uri": "http://attacker.example/acct/1"})
        for state in [dict(ACCOUNT, files={"../../credentials": "{}"}), dict(ACCOUNT, files={**ACCOUNT["files"], "logs/letsencrypt.log": "private"}), wrong_endpoint]:
            with self.subTest(state=state["files"].keys()), self.assertRaises(Refused):
                tls.account_files(state)

    def test_cli_never_prints_pem_token_or_private_failure_output(self):
        cloud = SecretManager()
        output = io.StringIO()
        env = {"AIDASH_GCP_CONFIG": json.dumps(CONFIG), "AIDASH_ACME_EMAIL": "maintainer@example.test", "CLOUDFLARE_CERTIFICATE_DNS_TOKEN": TOKEN}
        with patch.dict(os.environ, env), patch.object(sys, "argv", ["tls.py", "renew"]), patch("tls.run", side_effect=cloud.run), patch("tls.issue_certificate", return_value=self.bundles["healthy"]), redirect_stdout(output):
            tls.main()
        self.assertIn("renewed", output.getvalue())
        self.assertNotIn(TOKEN, output.getvalue())
        self.assertNotIn("BEGIN", output.getvalue())
        with patch.dict(os.environ, env), patch.object(sys, "argv", ["tls.py", "renew"]), patch("tls.renew", side_effect=RuntimeError(TOKEN + self.bundles["healthy"]["private_key_pem"])):
            with self.assertRaises(SystemExit) as failure:
                tls.main()
        self.assertNotIn(TOKEN, str(failure.exception))
        self.assertNotIn("BEGIN", str(failure.exception))


class PreflightTests(unittest.TestCase):
    def test_only_configured_default_ref_and_protected_automation_branch_supply_trusted_sha(self):
        def github(path):
            if "/branches/" not in path:
                return {"default_branch": "develop/0.1.0"}
            self.assertNotIn("/branches/main", path)
            return {"protected": True, "commit": {"sha": "a" * 40 if path.endswith("main") else "b" * 40}}

        env = {"GITHUB_REPOSITORY": "kent8192/aidash", "GITHUB_REF": "refs/heads/develop/0.1.0"}
        with patch.dict(os.environ, env), patch("tls.github", side_effect=github):
            self.assertEqual(tls.preflight(CONFIG), "b" * 40)
            with patch.dict(os.environ, {"GITHUB_REF": "refs/heads/feature/arbitrary"}), self.assertRaises(Refused):
                tls.preflight(CONFIG)
        with patch.dict(os.environ, env), patch("tls.github", side_effect=[{"default_branch": "develop/0.1.0"}, {"protected": False}]), self.assertRaises(Refused):
            tls.preflight(CONFIG)


if __name__ == "__main__":
    unittest.main()
