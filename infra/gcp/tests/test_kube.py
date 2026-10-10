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
from policy import IMAGE_KINDS, Refused

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

    def test_home_ledger_is_initialized_as_a_child_of_its_claim_root(self):
        # A fresh disk's root belongs to root; ledger initialization chmods its
        # directory, so the Home must be a child the application UID creates.
        output = dict(OUTPUT, server_service_account="server@fixture", worker_service_account="worker@fixture")
        release = {"images": {kind: f"registry.example/{kind}@sha256:" + "a" * 64 for kind in ("app", "control", "sandbox")}}
        settings = {"provider": None, "gcip": None}
        recovery = kube.app_values(self.cluster, "test", output, release, "a" * 40, settings)["memoryRecovery"]
        self.assertEqual(recovery, {"existingClaim": "memory-recovery", "subPath": "home",
                                    "directory": "/var/lib/aidash/memory-recovery/home"})
        # As rendered by the app chart from these values.
        mounts = [
            {"name": "memory-recovery", "mountPath": recovery["directory"], "subPath": recovery["subPath"]},
            {"name": "capabilities", "mountPath": "/var/lib/aidash/capabilities"},
        ]
        self.cluster.put({
            "kind": "Deployment", "metadata": {"name": "app-aidash-server", "namespace": "aidash-test"},
            "spec": {"replicas": 0, "template": {"metadata": {"labels": {}}, "spec": {"containers": [{
                "name": "aidash", "args": ["server"], "volumeMounts": mounts,
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
        self.assertEqual(container["env"], [{"name": "AIDASH_MEMORY_RECOVERY_DIR", "value": recovery["directory"]}])
        self.assertIn('--directory "$AIDASH_MEMORY_RECOVERY_DIR"', container["args"][0])


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

        def recorded_exec(namespace, target, container, *command):
            self.events.append(("edge", command[-1].rsplit("/", 1)[1]))
            return execute(namespace, target, container, *command)

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

    def desired(self, state):
        kube.desired_admission(self.cluster, "aidash-test", state)


if __name__ == "__main__":
    unittest.main()
