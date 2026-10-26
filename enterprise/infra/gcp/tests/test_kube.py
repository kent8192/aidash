# SPDX-FileCopyrightText: 2026 kent8192
# SPDX-License-Identifier: BUSL-1.1
# See enterprise/LICENSE.

"""Kubernetes boundary: private credentials, secret transport and preview disk handoff."""

import base64
import json
import os
from pathlib import Path
import stat
import subprocess
import sys
import tomllib
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import patch

import yaml

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "control"))
sys.path.insert(0, str(Path(__file__).resolve().parent))
import kube
from kubefake import PROJECT, FakeCluster
from policy import IMAGE_KINDS, Refused

HANDLE = f"projects/{PROJECT}/zones/us-central1-a/disks/aidash-preview-tls"
OUTPUT = {
    "hostname": "pr-2.aidash.run",
    "runtime_secret": "pr-2",
    "provider_credentials": None,
}
RUNTIME = {"AIDASH_OIDC_CLIENT_ID": "client", "AIDASH_OIDC_CLIENT_SECRET": "client-secret"}
RANDOM_HOST = "0123456789abcdef" * 2 + ".preview.aidash.run"
# PostgreSQL role passwords are 64 hex digits.
PASSWORDS = {"observer": "0" * 64, "owner": "1" * 64, "server": "2" * 64, "memory-recovery": "3" * 64}


class ClusterCommandTests(unittest.TestCase):
    def test_credentials_are_private_and_secret_material_never_reaches_argv_or_values(self):
        commands = []
        files = {}

        def run(*args, data=None, timeout=900, env=None):
            commands.append((args, data, env))
            if args[0] == "helm" and "upgrade" in args:
                path = Path(args[args.index("--values") + 1])
                self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o600)
                files["values"] = path.read_text()
            if args[:2] == ("kubectl", "--kubeconfig") and "get" in args and "secret" in args:
                return json.dumps({"metadata": {"resourceVersion": "1"}, "data": {
                    name: base64.b64encode(PASSWORDS.get(name, f"{name}-token").encode()).decode()
                    for name in kube.IDENTITY_TOKENS
                }}).encode()
            return b""

        with TemporaryDirectory() as directory, patch.object(kube, "run", run):
            cluster = kube.connect(
                {"project_id": PROJECT},
                {"name": "aidash", "location": "us-central1-a", "project_id": PROJECT, "pod_cidr": "10.44.0.0/14"},
                directory,
            )
            credentials = commands[0]
            self.assertEqual(credentials[0][:5], ("gcloud", "container", "clusters", "get-credentials", "aidash"))
            self.assertIn("--dns-endpoint", credentials[0])
            self.assertEqual(credentials[2], {"KUBECONFIG": str(Path(directory) / "kubeconfig")})
            self.assertEqual(stat.S_IMODE((Path(directory) / "kubeconfig").stat().st_mode), 0o600)
            settings = kube.materialize(cluster, "pr-2", PROJECT, OUTPUT, dict(RUNTIME, AIDASH_SECRET_OPENAI="sk-private"))
            cluster.upgrade("aidash-pr-2", "app", kube.APP_CHART, {"gcip": {"settings": settings["gcip"]}})
            kube.database_roles(cluster, "pr-2")
            kube.observer_role(cluster, "pr-2")
            kube.memory_role(cluster, "pr-2")
            self.assertEqual(os.listdir(directory), ["kubeconfig"])
        stdin = b"".join(data or b"" for _, data, _ in commands)
        argv = json.dumps([[str(arg) for arg in args] for args, _, _ in commands])
        for value in ("sk-private", "client-secret", "database-token", "runner-token", *PASSWORDS.values()):
            self.assertNotIn(value, argv)
            self.assertNotIn(value, files["values"])
        self.assertIn(base64.b64encode(b"sk-private"), stdin)
        self.assertTrue(all(args[1:3] == ("--kubeconfig", Path(directory) / "kubeconfig") for args, _, _ in commands[1:]))
        applied = [json.loads(data) for args, data, _ in commands if "apply" in args]
        self.assertTrue(all("--server-side" in args for args, _, _ in commands if "apply" in args))
        self.assertEqual(
            {item["metadata"]["name"] for item in applied},
            {"app-runtime", "app-migration", "app-memory-recovery", "app-runner", "env-postgres", "env-activity"},
        )
        # Role passwords reach PostgreSQL only through exec stdin: the legacy
        # ownership check, the roles, PGroonga, then the observer.
        execs = [(args, data) for args, data, _ in commands if "exec" in args]
        self.assertEqual(len(execs), 5)
        self.assertTrue(all("-i" in args for args, _ in execs))
        for password in PASSWORDS.values():
            self.assertIn(b"PASSWORD '" + password.encode() + b"'", stdin)


class RuntimeConfigurationTests(unittest.TestCase):
    def setUp(self):
        self.cluster = FakeCluster([])
        self.cluster.put({"kind": "Namespace", "metadata": {"name": "aidash-pr-2"}})

    def materialize(self, runtime, output=OUTPUT):
        return kube.materialize(self.cluster, "pr-2", PROJECT, output, runtime)

    def test_oidc_defaults_and_legacy_fingerprint_rename(self):
        legacy = "l" * 32
        self.assertEqual(self.materialize(dict(RUNTIME, AIDASH_SECRET_PROVIDER_FINGERPRINT=legacy))["gcip"], None)
        runtime = self.cluster.secret("aidash-pr-2", "app-runtime")
        self.assertEqual(runtime["AIDASH_OIDC_ISSUER"], "https://accounts.google.com")
        self.assertEqual(runtime["AIDASH_OIDC_PUBLIC_ORIGIN"], "https://pr-2.aidash.run")
        self.assertEqual(runtime["AIDASH_PROVIDER_FINGERPRINT_KEY"], legacy)
        self.assertNotIn("AIDASH_SECRET_PROVIDER_FINGERPRINT", runtime)
        self.assertEqual(runtime["NATS_URL"], "nats://env-environment-nats:4222")
        tokens = self.cluster.secret("aidash-pr-2", "aidash-identity")
        self.assertEqual(self.cluster.secret("aidash-pr-2", "env-activity")["AIDASH_CORE_RUNNER_TOKEN"], tokens["runner"])
        self.assertEqual(self.cluster.secret("aidash-pr-2", "app-runner"), {"AIDASH_CORE_RUNNER_TOKEN": tokens["runner"]})

    def test_common_auth_materializes_only_cloud_credentials_and_private_tls_files(self):
        runtime = {
            "AIDASH_OIDC_CLIENT_ID": "cloud", "AIDASH_OIDC_CLIENT_SECRET": "c" * 40,
            "dashboard": {
                "oidc": {"issuer": "https://auth.example.test", "public_origin": "https://pr-2.aidash.run"},
                "common_auth": {
                    "grpc_endpoint": "https://authentication.internal:9091", "server_domain": "authentication.internal",
                    "source_issuers": {"https://securetoken.google.com/project/tenants/shared": ""},
                },
            },
            "common_auth_tls": {"ca.pem": "-----BEGIN CERTIFICATE-----\nfixture-ca\n-----END CERTIFICATE-----",
                                "cloud.pem": "-----BEGIN CERTIFICATE-----\nfixture-client\n-----END CERTIFICATE-----",
                                "cloud-key.pem": "-----BEGIN PRIVATE KEY-----\nfixture-key\n-----END PRIVATE KEY-----"},
        }
        settings = self.materialize(runtime)
        self.assertIsNone(settings["gcip"])
        self.assertEqual(settings["common_auth"], "app-common-auth")
        stored = self.cluster.secret("aidash-pr-2", "app-runtime")
        self.assertEqual(stored["AIDASH_OIDC_ISSUER"], "https://auth.example.test")
        self.assertNotIn("common_auth_tls", stored)
        files = self.cluster.secret("aidash-pr-2", "app-common-auth")
        self.assertEqual({key: files[key] for key in runtime["common_auth_tls"]}, runtime["common_auth_tls"])
        dashboard = tomllib.loads(files["settings.toml"])["dashboard"]
        self.assertEqual(dashboard["oidc"]["client_secret"], "${AIDASH_OIDC_CLIENT_SECRET}")
        self.assertEqual(dashboard["common_auth"]["source_issuers"], runtime["dashboard"]["common_auth"]["source_issuers"])
        self.assertEqual(dashboard["common_auth"]["client_private_key_file"], "/run/auth-tls/cloud-key.pem")
        release = {"images": {kind: f"registry.example/{kind}@sha256:" + "a" * 64 for kind in IMAGE_KINDS}}
        output = dict(OUTPUT, server_service_account="server@example.test", worker_service_account="worker@example.test")
        values = kube.app_values(self.cluster, "test", output, release, "a" * 40, settings)
        self.assertEqual(values["commonAuth"], {"existingSecret": "app-common-auth"})
        for secret in (*runtime["common_auth_tls"].values(), runtime["AIDASH_OIDC_CLIENT_SECRET"]):
            self.assertNotIn(secret, json.dumps(values))
        for update in ({"dashboard": dict(runtime["dashboard"], gcip={})}, {"common_auth_tls": {}},
                       {"AIDASH_OIDC_CLIENT_ID": "marketplace"},
                       {"dashboard": dict(runtime["dashboard"], oidc={"issuer": "http://auth.internal", "public_origin": "https://pr-2.aidash.run"})}):
            with self.subTest(update=list(update)), self.assertRaises(Refused):
                self.materialize(dict(runtime, **update))

    def test_demo_data_is_enabled_for_nonproduction_servers_and_job_only(self):
        # The server Deployment reads app-runtime through envFrom; the copied
        # migration Job inherits the same envFrom. Both need the setting so a
        # sign-in can fulfil the Reservations that the Job loaded.
        self.materialize(RUNTIME)
        self.assertEqual(
            self.cluster.secret("aidash-pr-2", "app-runtime")["AIDASH_DEMO_DATA_ENABLED"],
            "true",
        )
        production = FakeCluster([])
        production.put({"kind": "Namespace", "metadata": {"name": "aidash-production"}})
        output = dict(OUTPUT, hostname="aidash.run", runtime_secret="production")
        kube.materialize(production, "production", PROJECT, output, RUNTIME)
        self.assertNotIn(
            "AIDASH_DEMO_DATA_ENABLED",
            production.secret("aidash-production", "app-runtime"),
        )

    def test_observer_never_receives_the_application_database_login(self):
        self.materialize(RUNTIME)
        tokens = self.cluster.secret("aidash-pr-2", "aidash-identity")
        self.assertEqual(set(tokens), {"database", "api", "runner", "observer", "owner", "server", "memory-recovery"})
        self.assertEqual(len({tokens[name] for name in ("database", "observer", "owner", "server", "memory-recovery")}), 5)
        maintenance = self.cluster.secret("aidash-pr-2", "app-memory-recovery")
        self.assertEqual(set(maintenance), {"DATABASE_URL", "AIDASH_API_TOKEN"})
        self.assertRegex(maintenance["AIDASH_API_TOKEN"], r"^[0-9a-f]{64}$")
        self.assertNotIn(maintenance["AIDASH_API_TOKEN"], tokens.values())
        self.assertIn("postgres://aidash_memory_recovery:", maintenance["DATABASE_URL"])
        for role in ("owner", "server", "observer"):
            self.assertNotIn(tokens[role], maintenance["DATABASE_URL"])
        activity = self.cluster.secret("aidash-pr-2", "env-activity")
        self.assertEqual(
            activity["DATABASE_URL"],
            f"postgres://aidash_observer:{tokens['observer']}@env-environment-postgres:5432/aidash_a",
        )
        self.assertNotIn(tokens["database"], json.dumps(activity))
        # Servers log in as the role row-level security binds; only the
        # migration Job receives the owner's login, and nobody the superuser's.
        runtime = self.cluster.secret("aidash-pr-2", "app-runtime")
        self.assertEqual(
            runtime["DATABASE_URL"],
            f"postgres://aidash_server:{tokens['server']}@env-environment-postgres:5432/aidash_a",
        )
        self.assertEqual(
            self.cluster.secret("aidash-pr-2", "app-migration"),
            {"DATABASE_URL": f"postgres://aidash_owner:{tokens['owner']}@env-environment-postgres:5432/aidash_a"},
        )
        for secret in ("database", "owner"):
            self.assertNotIn(tokens[secret], json.dumps(runtime))

    def test_existing_identity_gains_the_observer_token_once_and_keeps_its_tokens(self):
        existing = {name: name[0] * 64 for name in ("database", "api", "runner")}
        manifest = kube.secret("aidash-pr-2", "aidash-identity", existing, environment=False)
        manifest["metadata"]["resourceVersion"] = "7"
        self.cluster.put(manifest)
        applied = []
        apply = self.cluster.apply

        def recorded_apply(manifest):
            applied.append(manifest)
            apply(manifest)

        with patch.object(self.cluster, "apply", recorded_apply):
            self.materialize(RUNTIME)
        tokens = self.cluster.secret("aidash-pr-2", "aidash-identity")
        self.assertEqual({name: tokens[name] for name in existing}, existing)
        self.assertRegex(tokens["observer"], "^[0-9a-f]{64}$")
        # A concurrent addition fails on the precondition rather than being replaced.
        identity = next(item for item in applied if item["metadata"]["name"] == "aidash-identity")
        self.assertEqual(identity["metadata"]["resourceVersion"], "7")
        self.materialize(RUNTIME)
        self.assertEqual(self.cluster.secret("aidash-pr-2", "aidash-identity"), tokens)

    def test_gcip_is_public_chart_settings_without_oidc_defaults(self):
        gcip = {
            "project_id": PROJECT, "web_api_key": "public", "public_origin": "https://pr-2.aidash.run",
            "tenant_bindings": {"pool": "acme"}, "providers": {}, "password_sign_up": [],
            "sign_in_domains": {"acme.example": "pool"},
        }
        settings = self.materialize({"dashboard": {"gcip": gcip}})
        self.assertEqual(settings["gcip"], {"dashboard": {"gcip": gcip}})
        self.assertFalse(any(key.startswith("AIDASH_OIDC_") for key in self.cluster.secret("aidash-pr-2", "app-runtime")))

    def test_a_shared_pool_alone_is_a_managed_gcip_tenant_for_a_random_origin(self):
        output = dict(OUTPUT, hostname=RANDOM_HOST)
        gcip = {
            "project_id": PROJECT, "web_api_key": "public", "public_origin": "https://" + RANDOM_HOST,
            "tenant_bindings": {},
        }
        with self.assertRaises(Refused):
            self.materialize({"dashboard": {"gcip": gcip}}, output)
        shared = dict(gcip, shared_pool="shared-pool")
        settings = self.materialize({"dashboard": {"gcip": shared}}, output)
        self.assertEqual(settings["gcip"], {"dashboard": {"gcip": shared}})
        with self.assertRaises(Refused):
            self.materialize({"dashboard": {"gcip": dict(gcip, shared_pool=["shared-pool"])}}, output)

    def test_random_origin_refuses_legacy_oidc_and_dynamic_auth_helper(self):
        output = dict(OUTPUT, hostname=RANDOM_HOST)
        gcip = {
            "project_id": PROJECT, "web_api_key": "public", "public_origin": "https://" + RANDOM_HOST,
            "tenant_bindings": {"pool": "acme"},
        }
        for runtime in [RUNTIME, {"dashboard": {"gcip": dict(gcip, auth_helper="public_origin")}}]:
            with self.subTest(runtime=runtime), self.assertRaises(Refused):
                self.materialize(runtime, output)
        settings = self.materialize({"dashboard": {"gcip": gcip}}, output)
        release = {"images": {kind: f"registry.example/{kind}@sha256:" + "a" * 64 for kind in IMAGE_KINDS}}
        edge = kube.env_values("pr-2", output, release, settings)["edge"]
        self.assertEqual(edge["tlsSecret"], "aidash-preview-tls")
        self.assertEqual(edge["existingTlsClaim"], "")
        self.assertEqual(edge["firebaseProject"], "")

    def test_edge_serves_the_firebase_helper_only_when_the_settings_select_it(self):
        release = {"images": {kind: f"registry.example/{kind}@sha256:" + "a" * 64 for kind in IMAGE_KINDS}}
        gcip = {"project_id": PROJECT, "web_api_key": "public", "public_origin": "https://pr-2.aidash.run", "tenant_bindings": {}}
        for selected, expected in [(None, ""), ({}, ""), ({"auth_helper": "firebase"}, ""), ({"auth_helper": "public_origin"}, PROJECT)]:
            with self.subTest(selected=selected):
                dashboard = None if selected is None else {"gcip": dict(gcip, **selected)}
                settings = {"provider": None, "gcip": kube.gcip_settings(PROJECT, OUTPUT, dashboard)}
                self.assertEqual(kube.env_values("pr-2", OUTPUT, release, settings)["edge"]["firebaseProject"], expected)

    def test_unsupported_or_conflicting_runtime_configuration_is_refused(self):
        gcip = {"project_id": PROJECT, "web_api_key": "public", "public_origin": "https://pr-2.aidash.run", "tenant_bindings": {}}
        for runtime in [
            dict(RUNTIME, DATABASE_URL="postgres://elsewhere"),
            dict(RUNTIME, AIDASH_API_TOKEN="forged"),
            {"AIDASH_OIDC_CLIENT_ID": "client"},
            dict(RUNTIME, dashboard={"gcip": gcip}),
            {"dashboard": {"gcip": dict(gcip, public_origin="https://other.aidash.run")}},
            {"dashboard": {"gcip": dict(gcip, unexpected=True)}},
            {"dashboard": {"gcip": dict(gcip, auth_helper="auth.example.test")}},
            {"dashboard": {"gcip": dict(gcip, sign_in_domains=["acme.example"])}},
            dict(RUNTIME, AIDASH_SECRET_MULTILINE="a\nb"),
        ]:
            with self.subTest(runtime=runtime), self.assertRaises(Refused):
                self.materialize(runtime)
        self.assertNotIn(("secret", "aidash-pr-2", "app-runtime"), self.cluster.objects)

    def test_byok_descriptor_requires_a_stable_fingerprint_and_matching_broker(self):
        store = {"kind": "secret_manager", "byok_project_id": "aidash-byok-fixture", "environment_id": "pr-2"}
        broker = {"endpoint": "https://broker", "issuer": "https://issuer", "audience": "pr-2", "kid": "1"}
        descriptor = {"fingerprint_key": {"env": "AIDASH_PROVIDER_FINGERPRINT_KEY"}, "store": store, "broker": broker}
        output = dict(OUTPUT, provider_credentials=descriptor)
        with self.assertRaisesRegex(Refused, "fingerprint"):
            self.materialize(dict(RUNTIME, AIDASH_PROVIDER_FINGERPRINT_KEY="short"), output)
        with self.assertRaisesRegex(Refused, "broker"):
            self.materialize(dict(RUNTIME, AIDASH_PROVIDER_FINGERPRINT_KEY="f" * 32), dict(output, provider_credentials=dict(descriptor, broker=dict(broker, audience="test"))))
        settings = self.materialize(dict(RUNTIME, AIDASH_PROVIDER_FINGERPRINT_KEY="f" * 32), output)
        self.assertEqual(settings["provider"], descriptor)
        self.assertEqual(self.materialize(RUNTIME)["provider"], {"fingerprint_key": None, "store": None, "broker": None})


class PreviewTlsTests(unittest.TestCase):
    def setUp(self):
        self.calls = []
        self.cluster = FakeCluster(self.calls)
        self.context = patch.object(kube.time, "sleep")
        self.sleep = self.context.start()
        self.addCleanup(self.context.stop)
        kube.bind_preview_tls(self.cluster, "pr-1", HANDLE)
        self.cluster.workload("deployment", "aidash-pr-1", "env-environment-edge", 0)

    def test_rebinding_waits_until_the_previous_attachment_is_gone(self):
        self.cluster.attached["aidash-preview-tls"] = 3
        self.calls.clear()
        kube.bind_preview_tls(self.cluster, "pr-2", HANDLE)
        self.assertEqual(self.sleep.call_count, 3)
        self.assertEqual(self.calls, [
            ("delete", "pvc", "pr-1", "preview-tls"),
            ("patch_pv", "aidash-preview-tls", "aidash-pr-2"),
        ])
        volume = self.cluster.get("persistentvolume", "aidash-preview-tls")
        self.assertEqual(volume["spec"]["claimRef"], {"apiVersion": "v1", "kind": "PersistentVolumeClaim", "namespace": "aidash-pr-2", "name": "preview-tls"})
        self.assertEqual(self.cluster.get("persistentvolumeclaim", "preview-tls", "aidash-pr-2")["spec"]["volumeName"], "aidash-preview-tls")

    def test_running_edge_or_lingering_attachment_never_releases_the_disk(self):
        for running in [True, False]:
            with self.subTest(running=running):
                self.cluster.workload("deployment", "aidash-pr-1", "env-environment-edge", int(running))
                self.cluster.attached["aidash-preview-tls"] = 0 if running else 10 ** 6
                self.calls.clear()
                with patch.object(kube.time, "monotonic", side_effect=[0, 0, 1000, 1000]):
                    with self.assertRaisesRegex(RuntimeError, "still in use by aidash-pr-1"):
                        kube.bind_preview_tls(self.cluster, "pr-2", HANDLE)
                self.assertFalse(kube.preview_tls_available(self.cluster, "pr-2"))
                self.assertEqual(self.calls, [])
                self.assertEqual(self.cluster.get("persistentvolume", "aidash-preview-tls")["spec"]["claimRef"]["namespace"], "aidash-pr-1")

    def test_preview_volume_must_reference_the_managed_disk(self):
        with self.assertRaises(Refused):
            kube.bind_preview_tls(self.cluster, "pr-2", HANDLE.replace("aidash-preview-tls", "other"))


class WildcardTlsTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.files = TemporaryDirectory()
        cls.directory = Path(cls.files.name)
        for name, host in [("preview", "*.preview.aidash.run"), ("other", "*.aidash.run")]:
            subprocess.run([
                "openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "2",
                "-subj", "/CN=" + host, "-addext", "subjectAltName=DNS:" + host,
                "-out", str(cls.directory / (name + ".crt")), "-keyout", str(cls.directory / (name + ".key")),
            ], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        subprocess.run([
            "openssl", "x509", "-in", str(cls.directory / "preview.crt"),
            "-signkey", str(cls.directory / "preview.key"), "-days", "0",
            "-out", str(cls.directory / "expired.crt"),
        ], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    @classmethod
    def tearDownClass(cls):
        cls.files.cleanup()

    def test_only_valid_matching_wildcard_material_reaches_the_edge_secret(self):
        cluster = FakeCluster([])
        certificate = (self.directory / "preview.crt").read_text()
        key = (self.directory / "preview.key").read_text()
        kube.install_preview_tls(cluster, "pr-2", certificate, key)
        tls = cluster.get("secret", "aidash-preview-tls", "aidash-pr-2")
        self.assertEqual(tls["type"], "kubernetes.io/tls")
        self.assertEqual(cluster.secret("aidash-pr-2", "aidash-preview-tls"), {"tls.crt": certificate, "tls.key": key})
        for cert, private, identity in [
            ((self.directory / "other.crt").read_text(), (self.directory / "other.key").read_text(), "pr-2"),
            (certificate, (self.directory / "other.key").read_text(), "pr-2"),
            (certificate, key, "test"),
            ("not a certificate", key, "develop"),
            ((self.directory / "expired.crt").read_text(), key, "develop"),
        ]:
            with self.subTest(identity=identity, certificate=cert[:20]), self.assertRaises(Refused):
                kube.install_preview_tls(cluster, identity, cert, private)
        self.assertEqual(cluster.secret("aidash-pr-2", "aidash-preview-tls")["tls.key"], key)

    def test_rotation_restarts_an_existing_edge_without_exposing_key_in_its_spec(self):
        cluster = FakeCluster([])
        cluster.workload("deployment", "aidash-develop", "env-environment-edge", 1)
        certificate = (self.directory / "preview.crt").read_text()
        key = (self.directory / "preview.key").read_text()
        kube.install_preview_tls(cluster, "develop", certificate, key)
        first = cluster.get("deployment", "env-environment-edge", "aidash-develop")["spec"]["template"]["metadata"]["annotations"]
        with patch.object(cluster, "patch", wraps=cluster.patch) as restart, patch.object(cluster, "rollout", wraps=cluster.rollout) as readiness:
            self.assertFalse(kube.install_preview_tls(cluster, "develop", certificate, key))
        restart.assert_not_called()
        readiness.assert_called_once_with("aidash-develop", kube.EDGE)
        rotated = subprocess.check_output([
            "openssl", "x509", "-in", str(self.directory / "preview.crt"),
            "-signkey", str(self.directory / "preview.key"), "-set_serial", "2", "-days", "2",
        ], stderr=subprocess.DEVNULL).decode()
        self.assertTrue(kube.install_preview_tls(cluster, "develop", rotated, key))
        spec = cluster.get("deployment", "env-environment-edge", "aidash-develop")["spec"]
        self.assertNotEqual(spec["template"]["metadata"]["annotations"], first)
        self.assertNotIn(key, json.dumps(spec))

    def test_random_edge_renders_static_secret_only_in_caddy_and_no_tls_disk(self):
        release = {"images": {kind: f"registry.example/{kind}@sha256:" + "a" * 64 for kind in IMAGE_KINDS}}
        output = dict(OUTPUT, hostname=RANDOM_HOST)
        settings = {"gcip": {"dashboard": {"gcip": {"project_id": PROJECT}}}, "provider": None}
        rendered = subprocess.check_output([
            "helm", "template", "env", str(kube.ENV_CHART), "--namespace", "aidash-pr-2", "-f", "-",
        ], input=json.dumps(kube.env_values("pr-2", output, release, settings)).encode())
        objects = [obj for obj in yaml.safe_load_all(rendered) if obj]
        pod = next(obj for obj in objects if obj["kind"] == "Deployment")["spec"]["template"]["spec"]
        tls = next(volume for volume in pod["volumes"] if volume["name"] == "preview-tls")
        self.assertEqual(tls["secret"]["secretName"], "aidash-preview-tls")
        for container in pod["containers"]:
            mounts = [mount for mount in container["volumeMounts"] if mount["name"] == "preview-tls"]
            self.assertEqual(len(mounts), int(container["name"] == "caddy"))
            if mounts:
                self.assertTrue(mounts[0]["readOnly"])
        self.assertFalse(any(obj["kind"] == "PersistentVolumeClaim" for obj in objects))
        caddy = next(obj for obj in objects if obj["kind"] == "ConfigMap" and "Caddyfile" in obj["data"])["data"]["Caddyfile"]
        self.assertIn("auto_https off", caddy)
        self.assertIn("tls /etc/aidash-preview-tls/tls.crt /etc/aidash-preview-tls/tls.key", caddy)
        self.assertIn("@origin host " + RANDOM_HOST, caddy)
        self.assertIn("header >Referrer-Policy no-referrer", caddy)
        self.assertIn("respond 404", caddy)


class SealAndMigrationTests(unittest.TestCase):
    def setUp(self):
        self.calls = []
        self.cluster = FakeCluster(self.calls)
        for context in (patch.object(kube.time, "sleep"), patch.object(kube.time, "time", return_value=10000)):
            context.start()
            self.addCleanup(context.stop)
        self.cluster.put({"kind": "Namespace", "metadata": {"name": "aidash-test"}})
        self.cluster.put({"kind": "CronJob", "metadata": {"name": "env-environment-activity", "namespace": "aidash-test"}, "spec": {}})
        self.cluster.put({"kind": "ConfigMap", "metadata": {"name": "env-environment-activity", "namespace": "aidash-test"}})

    def workloads(self, edge, writers, dependencies=1):
        for name, replicas in (
            ("app-aidash-server", writers), ("app-aidash-worker", writers),
            ("app-execution-runner", dependencies), ("env-environment-edge", edge),
        ):
            self.cluster.workload("deployment", "aidash-test", name, replicas)
        for name in ("env-environment-postgres", "env-environment-nats"):
            self.cluster.workload("statefulset", "aidash-test", name, dependencies)

    def test_environment_without_live_pods_is_already_sealed(self):
        # A stop whose node pool apply failed leaves every workload at zero; there is
        # no edge to exec into and no producer, so sealing never restores writers.
        self.workloads(edge=0, writers=0, dependencies=0)
        self.assertTrue(kube.seal(self.cluster, "test"))
        self.assertTrue(kube.seal(self.cluster, "pr-9"))
        self.assertEqual(self.calls, [])
        self.assertEqual(self.cluster.replicas("aidash-test", "deployment", "app-aidash-server"), 0)

    def test_admission_is_closed_only_through_a_live_edge(self):
        self.workloads(edge=0, writers=1)
        self.assertTrue(kube.seal(self.cluster, "test"))
        self.assertEqual(self.calls, [
            ("scale", "test", "server", 0), ("scale", "test", "worker", 0), ("observe", "test"), ("activity", "test"),
        ])
        self.workloads(edge=1, writers=1)
        self.calls.clear()
        self.assertTrue(kube.seal(self.cluster, "test"))
        self.assertEqual(self.calls[0], ("admission", "test", "close"))

    def test_migration_pod_satisfies_the_copied_co_location_term(self):
        # As rendered by the app chart when server and worker share the RWO claims.
        term = {"topologyKey": "kubernetes.io/hostname", "labelSelector": {"matchLabels": {
            "app.kubernetes.io/instance": "app", "aidash.run/co-located": "true",
        }}}
        for colocated in (True, False):
            with self.subTest(colocated=colocated):
                labels = {"app.kubernetes.io/instance": "app", "app.kubernetes.io/component": "server"}
                affinity = {}
                if colocated:
                    labels["aidash.run/co-located"] = "true"
                    affinity = {"podAffinity": {"requiredDuringSchedulingIgnoredDuringExecution": [term]}}
                self.cluster.put({
                    "kind": "Deployment", "metadata": {"name": "app-aidash-server", "namespace": "aidash-test"},
                    "spec": {"replicas": 0, "template": {"metadata": {"labels": labels}, "spec": {
                        "affinity": affinity, "containers": [{"name": "aidash", "args": ["server"]}],
                    }}},
                })
                kube.migrate(self.cluster, "test", "a" * 40)
                (key,) = [key for key in self.cluster.objects if key[0] == "job"]
                pod = self.cluster.objects.pop(key)["spec"]["template"]
                selected = pod["metadata"]["labels"]
                # With the writers at zero, the Job Pod must match its own required term.
                for required in pod["spec"]["affinity"].get("podAffinity", {}).get("requiredDuringSchedulingIgnoredDuringExecution", []):
                    self.assertLessEqual(required["labelSelector"]["matchLabels"].items(), selected.items())
                # Never selected by the backend Service or the writer drain.
                self.assertEqual(selected["app.kubernetes.io/component"], "migration")
                self.assertEqual("aidash.run/co-located" in selected, colocated)

    def test_retry_waits_for_an_in_flight_migration_job_before_starting_another(self):
        # A reconciliation interrupted after creating its Job leaves it running.
        self.cluster.put({
            "kind": "Deployment", "metadata": {"name": "app-aidash-server", "namespace": "aidash-test"},
            "spec": {"replicas": 0, "template": {"metadata": {"labels": {}}, "spec": {
                "containers": [{"name": "aidash", "args": ["server"]}],
            }}},
        })
        previous = ("job", "aidash-test", "migrate-previous")
        self.cluster.put({"kind": "Job", "metadata": {"name": previous[2], "namespace": previous[1]}, "status": {"active": 1}})

        def jobs():
            return [key for key in self.cluster.objects if key[0] == "job"]

        # Still running at the deadline: no second Job shares its RWO claims.
        with patch.object(kube.time, "monotonic", side_effect=[0, 1000]), self.assertRaises(RuntimeError):
            kube.migrate(self.cluster, "test", "a" * 40)
        self.assertEqual(jobs(), [previous])

        def finish(_):
            self.assertEqual(jobs(), [previous])
            self.cluster.objects[previous]["status"] = {"failed": 1}

        with patch.object(kube.time, "sleep", side_effect=finish) as sleep:
            kube.migrate(self.cluster, "test", "a" * 40)
        sleep.assert_called_once()
        (created,) = [key for key in jobs() if key != previous]
        self.assertEqual(self.cluster.objects[created]["metadata"]["labels"]["app.kubernetes.io/component"], "migration")

    def test_protected_home_composes_independent_jobs_and_migration_network_access(self):
        output = dict(OUTPUT, server_service_account="server@fixture", worker_service_account="worker@fixture",
                      memory_recovery={"required": True, "history_bucket": "fixture-history", "archive_bucket": "fixture-archives",
                                       "backup_service_account": "backup@fixture", "prune_service_account": "prune@fixture"})
        release = {"images": {kind: f"registry.example/{kind}@sha256:" + "a" * 64 for kind in ("app", "control", "sandbox")}}
        values = kube.app_values(self.cluster, "test", output, release, "a" * 40, {"provider": None, "gcip": None})
        protection = values["memoryRecovery"]["protection"]
        self.assertTrue(protection["required"])
        self.assertEqual(protection["historyBucket"], "fixture-history")
        self.assertEqual(protection["archiveBucket"], "fixture-archives")
        self.assertEqual(protection["existingSecret"], "app-memory-recovery")
        self.assertEqual(protection["pruneServiceAccount"]["annotations"]["iam.gke.io/gcp-service-account"], "prune@fixture")
        full_release = {"images": {kind: f"registry.example/{kind}@sha256:" + "a" * 64 for kind in IMAGE_KINDS}}
        rendered = []
        for chart, name, chart_values in (
            (kube.APP_CHART, "app", values),
            (kube.ENV_CHART, "env", kube.env_values("test", output, full_release, {"provider": None, "gcip": None})),
        ):
            manifest = subprocess.check_output(["helm", "template", name, str(chart), "-f", "-"], input=json.dumps(chart_values).encode())
            rendered.extend(obj for obj in yaml.safe_load_all(manifest) if obj)
        ingress = next(obj for obj in rendered if obj["kind"] == "NetworkPolicy" and obj["metadata"]["name"].endswith("-dependencies"))["spec"]["ingress"]
        for job in (obj for obj in rendered if obj["kind"] == "CronJob" and "-memory-" in obj["metadata"]["name"]):
            labels = job["spec"]["jobTemplate"]["spec"]["template"]["metadata"]["labels"]
            allowed = set()
            for rule in ingress:
                for source in rule["from"]:
                    selector = source["podSelector"]
                    matches = selector.get("matchLabels", {}).items() <= labels.items() and all(
                        expression['operator'] == "In" and labels.get(expression["key"]) in expression["values"]
                        for expression in selector.get("matchExpressions", [])
                    )
                    if matches:
                        allowed.update(port["port"] for port in rule["ports"])
            self.assertEqual(allowed, {5432}, job["metadata"]["name"] + " needs PostgreSQL without unauthenticated NATS")
        self.cluster.put({
            "kind": "Deployment", "metadata": {"name": "app-aidash-server", "namespace": "aidash-test"},
            "spec": {"replicas": 0, "template": {"metadata": {"labels": {}}, "spec": {"containers": [{
                "name": "aidash", "env": [{"name": "AIDASH_MEMORY_RECOVERY_REQUIRED", "value": "true"}],
            }]}}},
        })
        kube.migrate(self.cluster, "test", "a" * 40)
        (job,) = [value for key,value in self.cluster.objects.items() if key[0] == "job"]
        self.assertEqual(job["spec"]["template"]["metadata"]["labels"]["aidash.run/metadata-server"], "allowed")
        with self.assertRaisesRegex(Refused, "protection"):
            kube.app_values(self.cluster, "production", {**output, "memory_recovery": {"required": False}}, release, "a" * 40, {"provider": None, "gcip": None})

    def test_existing_journal_claim_keeps_its_bound_storage_class(self):
        output = dict(OUTPUT, server_service_account="server@fixture", worker_service_account="worker@fixture")
        release = {"images": {kind: f"registry.example/{kind}@sha256:" + "a" * 64 for kind in ("app", "control", "sandbox")}}
        settings = {"provider": None, "gcip": None}
        fresh = kube.app_values(self.cluster, "test", output, release, "a" * 40, settings)
        self.assertEqual(fresh["execution"]["runner"]["journal"], {"storageClassName": kube.STORAGE_CLASS})
        self.cluster.put({
            "kind": "PersistentVolumeClaim",
            "metadata": {"name": kube.JOURNAL_CLAIM, "namespace": "aidash-test"},
            "spec": {"storageClassName": "former-default", "accessModes": ["ReadWriteOnce"]},
            "status": {"phase": "Bound"},
        })
        upgraded = kube.app_values(self.cluster, "test", output, release, "a" * 40, settings)
        self.assertEqual(upgraded["execution"]["runner"]["journal"], {"storageClassName": "former-default"})
        self.assertEqual(self.cluster.get("persistentvolumeclaim", kube.JOURNAL_CLAIM, "aidash-test")["spec"]["storageClassName"], "former-default")
        self.cluster.put({
            "kind": "PersistentVolumeClaim",
            "metadata": {"name": kube.JOURNAL_CLAIM, "namespace": "aidash-test"},
            "spec": {"accessModes": ["ReadWriteOnce"]},
            "status": {"phase": "Bound"},
        })
        static = kube.app_values(self.cluster, "test", output, release, "a" * 40, settings)
        self.assertEqual(static["execution"]["runner"]["journal"], {"storageClassName": ""})

    def test_production_keeps_its_retained_disks_on_regional_disks_in_two_zones(self):
        kube.prepare(self.cluster, "production")
        regional = self.cluster.get("storageclass", kube.STORAGE_CLASS)
        self.assertEqual(regional["parameters"], {
            "type": "pd-balanced", "replication-type": "regional-pd",
            "availability-class": "regional-hard-failover",
        })
        self.assertEqual(regional["allowedTopologies"], [{"matchLabelExpressions": [
            {"key": "topology.gke.io/zone", "values": ["us-central1-a", "us-central1-b"]},
        ]}])
        self.assertEqual(regional["reclaimPolicy"], "Retain")
        # Every claim the controller creates uses that class.
        for name, size in kube.CLAIMS.items():
            claim = kube.claim("aidash-production", name, size)
            self.assertEqual(claim["spec"]["storageClassName"], kube.STORAGE_CLASS)
        # Nonproduction's own cluster keeps zonal disks.
        zonal = FakeCluster([])
        kube.prepare(zonal, "test")
        self.assertEqual(zonal.get("storageclass", kube.STORAGE_CLASS)["parameters"], {"type": "pd-balanced"})
        self.assertNotIn("allowedTopologies", zonal.get("storageclass", kube.STORAGE_CLASS))

    def test_protect_selects_everything_but_the_ledger_claim_and_is_idempotent(self):
        kube.prepare(self.cluster, "production")
        # Without a backup plan the CRD may not exist, so nothing is applied for it.
        self.assertIsNone(self.cluster.get("protectedapplication", "aidash", "aidash-production"))
        kube.prepare(self.cluster, "production", protected=True)
        application = self.cluster.get("protectedapplication", "aidash", "aidash-production")
        self.assertEqual(application["apiVersion"], "gkebackup.gke.io/v1")
        self.assertEqual(application["spec"]["resourceSelection"], {"type": "Selector", "selector": {"matchExpressions": [
            {"key": "aidash.run/backup", "operator": "NotIn", "values": ["exclude"]},
        ]}})
        # A claim created before protection gains the label; no other claim does.
        self.assertEqual(self.cluster.get("persistentvolumeclaim", "memory-recovery", "aidash-production")["metadata"]["labels"], {"aidash.run/backup": "exclude"})
        self.assertNotIn("labels", self.cluster.get("persistentvolumeclaim", "capability-objects", "aidash-production")["metadata"])
        before = (application, self.cluster.get("persistentvolumeclaim", "memory-recovery", "aidash-production"))
        with patch.object(self.cluster, "patch", wraps=self.cluster.patch) as patched:
            kube.protect(self.cluster, "production")
        patched.assert_not_called()
        self.assertEqual(before, (self.cluster.get("protectedapplication", "aidash", "aidash-production"), self.cluster.get("persistentvolumeclaim", "memory-recovery", "aidash-production")))

    def test_protect_labels_a_claim_that_predates_the_label(self):
        namespace = "aidash-production"
        self.cluster.dynamic_claim(namespace, "memory-recovery")
        self.assertNotIn("labels", self.cluster.get("persistentvolumeclaim", "memory-recovery", namespace)["metadata"])
        kube.protect(self.cluster, "production")
        self.assertEqual(self.cluster.get("persistentvolumeclaim", "memory-recovery", namespace)["metadata"]["labels"], {"aidash.run/backup": "exclude"})

    def test_only_the_ledger_claim_is_created_with_the_exclusion_label(self):
        labelled = {name for name, size in kube.CLAIMS.items() if "labels" in kube.claim("aidash-production", name, size)["metadata"]}
        self.assertEqual(labelled, {"memory-recovery"})

    def test_production_is_never_created_on_zonal_disks(self):
        # Class parameters are immutable; a zonal class would keep the zone a
        # single point of failure, so production refuses to start on it.
        kube.prepare(self.cluster, "test")
        with self.assertRaisesRegex(Refused, "differs"):
            kube.prepare(self.cluster, "production")
        single = FakeCluster([])
        single.zones = ["us-central1-a"]
        with self.assertRaisesRegex(Refused, "two zones"):
            kube.prepare(single, "production")

    def test_production_disk_holders_are_statefulsets_under_stateful_ha(self):
        output = dict(OUTPUT, hostname="aidash.run", runtime_secret="production",
                      server_service_account="server@fixture", worker_service_account="worker@fixture",
                      memory_recovery={"required": True, "history_bucket": "fixture-history", "archive_bucket": "fixture-archives", "backup_service_account": "backup@fixture", "prune_service_account": "prune@fixture"})
        release = {"images": {kind: f"registry.example/{kind}@sha256:" + "a" * 64 for kind in IMAGE_KINDS}}
        settings = {"provider": None, "gcip": None}
        failover = {"workloadKind": "StatefulSet", "highAvailability": {"enabled": True}}
        for values in (kube.app_values(self.cluster, "production", output, release, "a" * 40, settings),
                       kube.env_values("production", output, release, settings)):
            self.assertEqual({key: values[key] for key in failover}, failover)
        for values in (kube.app_values(self.cluster, "test", OUTPUT | output, release, "a" * 40, settings),
                       kube.env_values("test", OUTPUT | output, release, settings)):
            self.assertFalse(failover.keys() & values.keys())
        for workload in (kube.SERVER, kube.WORKER, kube.RUNNER, kube.EDGE):
            self.assertEqual(kube.target("production", workload).split("/")[0], "statefulset")
            self.assertEqual(kube.target("test", workload), workload)

    def test_home_ledger_is_initialized_as_a_child_of_its_claim_root(self):
        # A fresh disk's root belongs to root; ledger initialization chmods its
        # directory, so the Home must be a child the application UID creates.
        output = dict(OUTPUT, server_service_account="server@fixture", worker_service_account="worker@fixture")
        release = {"images": {kind: f"registry.example/{kind}@sha256:" + "a" * 64 for kind in ("app", "control", "sandbox")}}
        settings = {"provider": None, "gcip": None}
        recovery = kube.app_values(self.cluster, "test", output, release, "a" * 40, settings)["memoryRecovery"]
        self.assertEqual(recovery, {"existingClaim": "memory-recovery", "subPath": "home",
                                    "directory": "/var/lib/aidash/memory-recovery/home", "protection": {"required": False}})
        # As rendered by the app chart from these values.
        mounts = [
            {"name": "memory-recovery", "mountPath": recovery["directory"], "subPath": recovery["subPath"]},
            {"name": "capabilities", "mountPath": "/var/lib/aidash/capabilities"},
        ]
        self.cluster.put({
            "kind": "Deployment", "metadata": {"name": "app-aidash-server", "namespace": "aidash-test"},
            "spec": {"replicas": 0, "template": {"metadata": {"labels": {}}, "spec": {"containers": [{
                "name": "aidash", "args": ["server"], "volumeMounts": mounts,
                "envFrom": [{"secretRef": {"name": "app-runtime"}}],
                "env": [{"name": "AIDASH_MEMORY_RECOVERY_DIR", "value": recovery["directory"]}],
            }]}}},
        })
        kube.migrate(self.cluster, "test", "a" * 40)
        (key,) = [key for key in self.cluster.objects if key[0] == "job"]
        (container,) = self.cluster.objects.pop(key)["spec"]["template"]["spec"]["containers"]
        # The Job mounts the claim root, so init-if-missing creates `home` itself.
        self.assertEqual(container["volumeMounts"], [
            {"name": "memory-recovery", "mountPath": "/var/lib/aidash/memory-recovery"},
            {"name": "capabilities", "mountPath": "/var/lib/aidash/capabilities"},
        ])
        # Unlike the server, the Job logs in as the owner of every table.
        self.assertEqual(container["env"], [
            {"name": "AIDASH_MEMORY_RECOVERY_DIR", "value": recovery["directory"]},
            {"name": "DATABASE_URL", "valueFrom": {"secretKeyRef": {"name": "app-migration", "key": "DATABASE_URL"}}},
        ])
        # The copied server template keeps reading app-runtime, which carries
        # AIDASH_DEMO_DATA_ENABLED for a non-production Environment.
        self.assertEqual(container["envFrom"], [{"secretRef": {"name": "app-runtime"}}])
        self.assertIn('--directory "$AIDASH_MEMORY_RECOVERY_DIR"', container["args"][0])

    def migration_job(self, identity, people=None, fork=False):
        """Run the migration Job and return it with every Secret applied meanwhile."""
        namespace = kube.namespace_of(identity)
        kind, name = kube.target(identity, kube.SERVER).split("/", 1)
        self.cluster.put({
            "kind": kind.capitalize(), "metadata": {"name": name, "namespace": namespace},
            "spec": {"replicas": 0, "template": {"metadata": {"labels": {}}, "spec": {"containers": [{
                "name": "aidash", "args": ["server"], "env": [], "volumeMounts": [],
                "envFrom": [{"secretRef": {"name": "app-runtime"}}],
            }]}}},
        })
        applied = []
        apply = self.cluster.apply
        self.cluster.apply = lambda manifest: (applied.append(manifest), apply(manifest))[1]
        kube.migrate(self.cluster, identity, "a" * 40, people, fork)
        (key,) = [key for key in self.cluster.objects if key[0] == "job"]
        job = self.cluster.objects.pop(key)
        return job["spec"]["template"]["spec"], [item for item in applied if item["kind"] == "Secret"]

    def test_production_migration_loads_no_demo_data(self):
        spec, applied = self.migration_job("production", people='{"people": []}')
        (container,) = spec["containers"]
        self.assertNotIn("demo-data", container["args"][0])
        self.assertFalse(any(item["name"] == "AIDASH_DEMO_DATA_ENABLED" for item in container["env"]))
        self.assertNotIn("volumes", spec)
        self.assertEqual(applied, [])
        self.assertIsNone(self.cluster.get("secret", kube.DEMO_SECRET, "aidash-production"))

    def test_other_environments_load_the_structure_without_people(self):
        spec, applied = self.migration_job("test")
        (container,) = spec["containers"]
        self.assertTrue(container["args"][0].endswith(" && aidash demo-data --from /app/deploy/demo-data/structure.json"))
        # The Job inherits the server's app-runtime envFrom, which
        # `materialize` gives the enabled flag to on non-production.
        self.assertEqual(container["envFrom"], [{"secretRef": {"name": "app-runtime"}}])
        self.assertNotIn("volumes", spec)
        self.assertEqual(container["volumeMounts"], [])
        self.assertEqual(applied, [])

    def test_people_reach_only_the_migration_job_and_are_removed_afterwards(self):
        people = '{"people": [{"email": "alex@example.test"}]}'
        spec, applied = self.migration_job("test", people=people)
        (container,) = spec["containers"]
        self.assertTrue(container["args"][0].endswith(
            " && aidash demo-data --from /app/deploy/demo-data/structure.json"
            " --people /run/aidash/demo-data/people.json"
        ))
        self.assertEqual(spec["volumes"], [{"name": "demo-data", "secret": {"secretName": "app-demo-data"}}])
        self.assertEqual(container["volumeMounts"], [
            {"name": "demo-data", "mountPath": "/run/aidash/demo-data", "readOnly": True},
        ])
        (manifest,) = applied
        self.assertEqual(manifest["metadata"], {"name": "app-demo-data", "namespace": "aidash-test"})
        self.assertEqual(base64.b64decode(manifest["data"]["people.json"]).decode(), people)
        # The server and worker templates are untouched, and the people do not stay behind.
        server = self.cluster.get("deployment", "app-aidash-server", "aidash-test")["spec"]["template"]["spec"]
        self.assertNotIn("volumes", server)
        self.assertIsNone(self.cluster.get("secret", "app-demo-data", "aidash-test"))

    def test_fork_never_receives_people_and_loses_a_stale_secret(self):
        self.cluster.put(kube.secret("aidash-test", "app-demo-data", {"people.json": "{}"}, environment=False))
        spec, applied = self.migration_job("test", people='{"people": []}', fork=True)
        (container,) = spec["containers"]
        self.assertNotIn("--people", container["args"][0])
        self.assertIn("aidash demo-data --from", container["args"][0])
        self.assertNotIn("volumes", spec)
        self.assertEqual(applied, [])
        self.assertIsNone(self.cluster.get("secret", "app-demo-data", "aidash-test"))

    def test_unconfigured_environment_loses_a_stale_secret(self):
        self.cluster.put(kube.secret("aidash-test", "app-demo-data", {"people.json": "{}"}, environment=False))
        spec, _ = self.migration_job("test")
        self.assertNotIn("volumes", spec)
        self.assertIsNone(self.cluster.get("secret", "app-demo-data", "aidash-test"))

    def test_failed_migration_still_removes_the_people(self):
        self.cluster.put({
            "kind": "Deployment", "metadata": {"name": "app-aidash-server", "namespace": "aidash-test"},
            "spec": {"replicas": 0, "template": {"metadata": {"labels": {}}, "spec": {"containers": [{"name": "aidash"}]}}},
        })
        with patch.object(kube, "wait_job", return_value=False), self.assertRaises(RuntimeError):
            kube.migrate(self.cluster, "test", "a" * 40, '{"people": []}')
        self.assertIsNone(self.cluster.get("secret", "app-demo-data", "aidash-test"))


class MemoryScheduleTests(unittest.TestCase):
    def setUp(self):
        self.cluster = FakeCluster([])
        self.namespace = "aidash-test"
        self.jobs = ("app-aidash-memory-backup", "app-aidash-memory-prune")
        self.settings = {"provider": None, "gcip": None}
        self.output = dict(OUTPUT, server_service_account="server@fixture", worker_service_account="worker@fixture",
                           memory_recovery={"required": True, "history_bucket": "fixture-history", "archive_bucket": "fixture-archives",
                                            "backup_service_account": "backup@fixture", "prune_service_account": "prune@fixture"})
        self.release = {"images": {kind: f"registry.example/{kind}@sha256:" + "a" * 64 for kind in IMAGE_KINDS}}
        kube.prepare(self.cluster, "test")
        kube.materialize(self.cluster, "test", PROJECT, self.output, RUNTIME)
        upgrade = self.cluster.upgrade

        def install(namespace, name, chart, values):
            upgrade(namespace, name, chart, values)
            if chart == kube.APP_CHART:
                # Use the actual chart's schedules, including upgrade resets.
                manifest = subprocess.check_output(["helm", "template", name, str(chart), "--namespace", namespace, "-f", "-"],
                                                   input=json.dumps(values).encode())
                for obj in yaml.safe_load_all(manifest):
                    if obj and obj["kind"] == "CronJob":
                        obj["metadata"]["namespace"] = namespace
                        self.cluster.put(obj)

        context = patch.object(self.cluster, "upgrade", install)
        context.start()
        self.addCleanup(context.stop)

    def assert_suspended(self, suspended):
        for name in self.jobs:
            self.assertIs(self.cluster.get("cronjob", name, self.namespace)["spec"].get("suspend"), suspended, name)

    def deploy(self):
        kube.deploy(self.cluster, "test", self.output, self.release, "a" * 40, self.settings)

    def test_schedules_wait_for_migrations_and_maintenance_role(self):
        for step in ("migrate", "memory_role"):
            original = getattr(kube, step)

            def initializing(*args, _original=original, **kwargs):
                self.assert_suspended(True)
                return _original(*args, **kwargs)

            context = patch.object(kube, step, initializing)
            context.start()
            self.addCleanup(context.stop)
        self.deploy()
        self.assert_suspended(False)

    def test_failed_migration_or_maintenance_role_leaves_schedules_suspended(self):
        for step in ("migrate", "memory_role"):
            with self.subTest(step=step):
                self.cluster.failures.clear()
                self.cluster.fail((step, "test"), RuntimeError("initialization failed"))
                with self.assertRaisesRegex(RuntimeError, "initialization failed"):
                    self.deploy()
                self.assert_suspended(True)

    def test_reload_resumes_only_after_successful_maintenance_role(self):
        self.deploy()
        original = kube.memory_role

        def initialize(*args):
            self.assert_suspended(True)
            return original(*args)

        with patch.object(kube, "memory_role", initialize):
            kube.reload(self.cluster, "test", self.output, self.settings)
        self.assert_suspended(False)
        self.cluster.fail(("memory_role", "test"), RuntimeError("role failed"))
        with self.assertRaisesRegex(RuntimeError, "role failed"):
            kube.reload(self.cluster, "test", self.output, self.settings)
        self.assert_suspended(True)

    def test_stop_suspends_every_schedule_before_shutting_down_dependencies(self):
        self.deploy()
        scale = self.cluster.scale

        def shutdown(*args):
            self.assert_suspended(True)
            self.assertTrue(self.cluster.get("cronjob", kube.ACTIVITY, self.namespace)["spec"]["suspend"])
            return scale(*args)

        with patch.object(self.cluster, "scale", shutdown):
            kube.stop(self.cluster, "test")
        self.assert_suspended(True)


class DatabaseRoleTests(unittest.TestCase):
    """Migrations log in as the owner; servers as a role row-level security binds (#262)."""

    def setUp(self):
        self.calls = []
        self.cluster = FakeCluster(self.calls)
        self.cluster.workload("statefulset", "aidash-test", "env-environment-postgres", 1)

    def test_roles_are_unprivileged_and_the_server_holds_only_dml(self):
        sql = kube.database_roles_sql("1" * 64, "2" * 64)
        for role in ("aidash_owner", "aidash_server"):
            (attributes,) = [line for line in sql.splitlines() if line.startswith(f"ALTER ROLE {role} ")]
            for attribute in ("NOSUPERUSER", "NOCREATEDB", "NOCREATEROLE", "NOBYPASSRLS"):
                self.assertIn(attribute, attributes)
        self.assertIn("ALTER DATABASE aidash_a OWNER TO aidash_owner;", sql)
        grants = [line for line in sql.splitlines() if line.startswith("ALTER DEFAULT PRIVILEGES")]
        self.assertEqual(grants, [
            "ALTER DEFAULT PRIVILEGES FOR ROLE aidash_owner GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO aidash_server;",
            "ALTER DEFAULT PRIVILEGES FOR ROLE aidash_owner GRANT USAGE, SELECT ON SEQUENCES TO aidash_server;",
        ])
        # TRUNCATE skips row-level security, so it is never granted.
        self.assertNotIn("TRUNCATE", sql)
        self.assertNotIn("GRANT ALL", sql)
        with self.assertRaises(Refused):
            kube.database_roles_sql("owner'; DROP ROLE aidash_owner; --", "2" * 64)

    def test_a_database_that_predates_the_roles_is_refused_before_any_change(self):
        self.cluster.legacy_tables.add("aidash-test")
        with self.assertRaisesRegex(Refused, "recreate the Environment"):
            kube.database_roles(self.cluster, "test")
        self.assertEqual(self.calls, [("legacy_ownership", "test")])
        self.assertNotIn("aidash-test", self.cluster.role_sql)

    def test_pgroonga_start_is_retried_on_fresh_sessions(self):
        attempts = []
        exec_ = self.cluster.exec

        def flaky(namespace, target, container, *command, data=None):
            if b"pgroonga" in (data or b"") and len(attempts) < 2:
                attempts.append(data)
                raise RuntimeError("kubectl failed (exit 1); no private command output was logged")
            return exec_(namespace, target, container, *command, data=data)

        with patch.object(self.cluster, "exec", flaky), patch.object(kube.time, "sleep"):
            kube.database_roles(self.cluster, "test")
        self.assertEqual(len(attempts), 2)
        self.assertIn(("pgroonga", "test"), self.calls)


class AdmissionTests(unittest.TestCase):
    """The edge restores the controller-owned desired state when it restarts."""

    def setUp(self):
        self.calls = []
        self.cluster = FakeCluster(self.calls)
        self.events = []
        apply, execute = self.cluster.apply, self.cluster.exec

        def recorded_apply(manifest):
            if manifest["metadata"]["name"] == kube.ADMISSION:
                self.events.append(("state", manifest["data"]["state"]))
            apply(manifest)

        def recorded_exec(namespace, target, container, *command, data=None):
            if container == "admission":
                self.events.append(("edge", command[-1].rsplit("/", 1)[1]))
            return execute(namespace, target, container, *command, data=data)

        for context in (
            patch.object(self.cluster, "apply", recorded_apply), patch.object(self.cluster, "exec", recorded_exec),
            patch.object(kube.time, "sleep"), patch.object(kube.time, "time", return_value=10000),
        ):
            context.start()
            self.addCleanup(context.stop)
        self.cluster.put({"kind": "Namespace", "metadata": {"name": "aidash-test"}})
        self.cluster.workload("deployment", "aidash-test", "env-environment-edge", 1)

    def state(self):
        return self.cluster.get("configmap", kube.ADMISSION, "aidash-test")["data"]["state"]

    def test_close_records_first_and_open_records_last(self):
        kube.admission(self.cluster, "test", "close")
        kube.admission(self.cluster, "test", "open")
        self.assertEqual(self.events, [("state", "closed"), ("edge", "close"), ("edge", "open"), ("state", "open")])
        self.assertEqual(self.state(), "open")

    def test_partial_failure_leaves_a_restarted_edge_closed(self):
        kube.admission(self.cluster, "test", "open")
        self.cluster.fail(("admission", "test", "close"), RuntimeError("exec unavailable"))
        with self.assertRaisesRegex(RuntimeError, "exec unavailable"):
            kube.admission(self.cluster, "test", "close")
        self.assertEqual(self.state(), "closed")
        self.cluster.fail(("admission", "test", "open"), RuntimeError("exec unavailable"))
        with self.assertRaisesRegex(RuntimeError, "exec unavailable"):
            kube.admission(self.cluster, "test", "open")
        self.assertEqual(self.state(), "closed")

    def test_close_without_an_edge_pod_records_the_closed_state_only(self):
        self.cluster.workload("deployment", "aidash-test", "env-environment-edge", 0)
        kube.admission(self.cluster, "test", "close")
        self.assertEqual(self.events, [("state", "closed")])
        with self.assertRaises(RuntimeError):
            kube.admission(self.cluster, "test", "open")
        self.assertEqual(self.state(), "closed")

    def test_seal_stop_and_deploy_never_leave_an_open_state_behind(self):
        for name in ("app-aidash-server", "app-aidash-worker", "app-execution-runner"):
            self.cluster.workload("deployment", "aidash-test", name, 1)
        self.cluster.put({"kind": "CronJob", "metadata": {"name": kube.ACTIVITY, "namespace": "aidash-test"}, "spec": {}})
        self.cluster.put({"kind": "ConfigMap", "metadata": {"name": kube.ACTIVITY, "namespace": "aidash-test"}})
        kube.admission(self.cluster, "test", "open")
        self.events.clear()
        self.assertTrue(kube.seal(self.cluster, "test"))
        # Closed durably before the live gate, and both before any writer drains.
        self.assertEqual(self.events, [("state", "closed"), ("edge", "close")])
        self.assertLess(self.calls.index(("admission", "test", "close")), self.calls.index(("scale", "test", "server", 0)))
        # A forced stop skips the seal; it still leaves the next edge closed.
        kube.admission(self.cluster, "test", "open")
        kube.stop(self.cluster, "test")
        self.assertEqual(self.state(), "closed")
        # A stopped Environment seals without Pods and keeps the state closed.
        self.desired("open")
        self.assertTrue(kube.seal(self.cluster, "test"))
        self.assertEqual(self.state(), "closed")
        # A deploy starts every new edge closed until readiness opens it.
        self.desired("open")
        release = {"images": {kind: f"registry.example/{kind}@sha256:" + "a" * 64 for kind in IMAGE_KINDS}}
        output = dict(OUTPUT, server_service_account="server@fixture", worker_service_account="worker@fixture")
        self.events.clear()
        kube.deploy(self.cluster, "test", output, release, "a" * 40, {"provider": None, "gcip": None})
        self.assertEqual(self.events, [("state", "closed")])
        self.assertEqual(self.state(), "closed")
        # Roles exist before the migration Job logs in as the owner; the observer's
        # role is granted only after migration created its tables.
        self.assertLess(self.calls.index(("legacy_ownership", "test")), self.calls.index(("database_roles", "test")))
        self.assertLess(self.calls.index(("database_roles", "test")), self.calls.index(("pgroonga", "test")))
        self.assertLess(self.calls.index(("pgroonga", "test")), self.calls.index(("migrate", "test")))
        self.assertLess(self.calls.index(("migrate", "test")), self.calls.index(("observer_role", "test")))
        tokens = self.cluster.secret("aidash-test", "aidash-identity")
        self.assertEqual(self.cluster.role_sql["aidash-test"], kube.database_roles_sql(tokens["owner"], tokens["server"]))
        sql = self.cluster.sql["aidash-test"]
        password = self.cluster.secret("aidash-test", "aidash-identity")["observer"]
        self.assertEqual(sql, kube.observer_sql(password))
        self.assertIn("GRANT pg_read_all_stats TO aidash_observer;", sql)
        self.assertIn("NOSUPERUSER", sql)
        granted = next(line for line in sql.splitlines() if line.startswith("GRANT SELECT ON TABLE"))
        self.assertEqual(granted, "GRANT SELECT ON TABLE " + ", ".join(kube.OBSERVER_TABLES) + " TO aidash_observer;")
        self.assertNotIn("GRANT ALL", sql)

    def desired(self, state):
        kube.desired_admission(self.cluster, "aidash-test", state)


if __name__ == "__main__":
    unittest.main()
