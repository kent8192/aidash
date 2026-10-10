"""Kubernetes boundary: private credentials, secret transport and preview disk handoff."""

import base64
import json
import os
from pathlib import Path
import stat
import sys
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "control"))
sys.path.insert(0, str(Path(__file__).resolve().parent))
import kube
from kubefake import PROJECT, FakeCluster
from policy import Refused

HANDLE = f"projects/{PROJECT}/zones/us-central1-a/disks/aidash-preview-tls"
OUTPUT = {
    "hostname": "pr-2.aidash.run",
    "runtime_secret": "pr-2",
    "provider_credentials": None,
}
RUNTIME = {"AIDASH_OIDC_CLIENT_ID": "client", "AIDASH_OIDC_CLIENT_SECRET": "client-secret"}


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
                return json.dumps({"data": {
                    name: base64.b64encode(f"{name}-token".encode()).decode()
                    for name in ("database", "api", "runner")
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
            self.assertEqual(os.listdir(directory), ["kubeconfig"])
        stdin = b"".join(data or b"" for _, data, _ in commands)
        argv = json.dumps([[str(arg) for arg in args] for args, _, _ in commands])
        for value in ("sk-private", "client-secret", "database-token", "runner-token"):
            self.assertNotIn(value, argv)
            self.assertNotIn(value, files["values"])
        self.assertIn(base64.b64encode(b"sk-private"), stdin)
        self.assertTrue(all(args[1:3] == ("--kubeconfig", Path(directory) / "kubeconfig") for args, _, _ in commands[1:]))
        applied = [json.loads(data) for args, data, _ in commands if "apply" in args]
        self.assertTrue(all("--server-side" in args for args, _, _ in commands if "apply" in args))
        self.assertEqual({item["metadata"]["name"] for item in applied}, {"app-runtime", "app-runner", "env-postgres", "env-activity"})


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

    def test_gcip_is_public_chart_settings_without_oidc_defaults(self):
        gcip = {
            "project_id": PROJECT, "web_api_key": "public", "public_origin": "https://pr-2.aidash.run",
            "tenant_bindings": {"pool": "acme"}, "providers": {}, "password_sign_up": [],
        }
        settings = self.materialize({"dashboard": {"gcip": gcip}})
        self.assertEqual(settings["gcip"], {"dashboard": {"gcip": gcip}})
        self.assertFalse(any(key.startswith("AIDASH_OIDC_") for key in self.cluster.secret("aidash-pr-2", "app-runtime")))

    def test_unsupported_or_conflicting_runtime_configuration_is_refused(self):
        gcip = {"project_id": PROJECT, "web_api_key": "public", "public_origin": "https://pr-2.aidash.run", "tenant_bindings": {}}
        for runtime in [
            dict(RUNTIME, DATABASE_URL="postgres://elsewhere"),
            dict(RUNTIME, AIDASH_API_TOKEN="forged"),
            {"AIDASH_OIDC_CLIENT_ID": "client"},
            dict(RUNTIME, dashboard={"gcip": gcip}),
            {"dashboard": {"gcip": dict(gcip, public_origin="https://other.aidash.run")}},
            {"dashboard": {"gcip": dict(gcip, unexpected=True)}},
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


if __name__ == "__main__":
    unittest.main()
