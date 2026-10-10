"""Exercise complete reconciliation with cloud boundaries replaced, not the policy."""

from contextlib import ExitStack, contextmanager
from copy import deepcopy
import json
import os
from pathlib import Path
import sys
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "control"))
sys.path.insert(0, str(Path(__file__).resolve().parent))
import controller
from cloud import Terraform as NativeTerraform
from kubefake import PROJECT, FakeCluster
from policy import IMAGE_KINDS, transition


SHA = "a" * 40
CONFIG = {"repository": "kent8192/aidash", "project_id": PROJECT}
RUNTIME = {
    "AIDASH_OIDC_CLIENT_ID": "client",
    "AIDASH_OIDC_CLIENT_SECRET": "client-secret",
    "AIDASH_PROVIDER_FINGERPRINT_KEY": "f" * 32,
}
IMAGES = {
    kind: f"us-central1-docker.pkg.dev/{PROJECT}/aidash/{kind}@sha256:{'d' * 64}"
    for kind in IMAGE_KINDS
}


def descriptor(identity, broker=True, kid="version-1"):
    return {
        "fingerprint_key": {"env": "AIDASH_PROVIDER_FINGERPRINT_KEY"},
        "store": {"kind": "secret_manager", "byok_project_id": "aidash-byok-fixture", "environment_id": identity},
        "broker": {"endpoint": "https://broker.run.app/api/v1", "issuer": "https://broker.run.app", "audience": identity, "kid": kid} if broker else None,
    }


def seal(identity):
    return [
        ("admission", identity, "close"),
        ("scale", identity, "server", 0),
        ("scale", identity, "worker", 0),
        ("observe", identity),
        ("activity", identity),
    ]


def unseal(identity):
    return [("scale", identity, "server", 1), ("scale", identity, "worker", 1), ("admission", identity, "open")]


class ConfigurationTests(unittest.TestCase):
    def test_existing_lifecycle_config_does_not_require_byok(self):
        config = dict.fromkeys(
            ("project_id", "state_bucket", "release_bucket", "cloudflare_zone_id",
             "deploy_service_account", "repository", "develop_branch"),
            "fixture",
        )
        config["domain"] = "aidash.run"
        with TemporaryDirectory() as directory, patch.object(
            controller, "CONFIG", Path(directory) / "absent.json"
        ):
            for byok in [None, "aidash-byok-fixture"]:
                value = dict(config)
                if byok is not None:
                    value["byok_project_id"] = byok
                with patch.dict(os.environ, {"AIDASH_GCP_CONFIG": json.dumps(value)}):
                    self.assertEqual(controller.load_config(), value)


class MemoryStore:
    def __init__(self):
        self.state = {"environments": {}}
        self.locked = False

    def read(self, key):
        return deepcopy(self.state), "1"

    def mutate(self, callback):
        self.state, value = callback(deepcopy(self.state))
        return value

    @contextmanager
    def lock(self, wait_seconds=0):
        if self.locked:
            raise RuntimeError("lifecycle lock is busy")
        self.locked = True
        try:
            yield
        finally:
            self.locked = False


class CloudFixture:
    def __init__(self):
        self.managed = {}
        self.plans = []
        self.configuration = {}
        self.brokers = {}
        self.gcip = {}
        self.resource_changes = []
        self.output_changes = {}
        self.applied = False

    def broker_configuration_changed(self, store, environments):
        return self.brokers != self.broker_configuration(environments)

    def broker_configuration_in_state(self, store):
        return deepcopy(self.brokers)

    def broker_configuration(self, environments):
        return {
            key: dict(value, enabled=value.get("enabled") or False)
            for key, value in self.configuration.get("credential_brokers", {}).items()
            if key in environments
        }

    def configuration_in_state(self, store):
        return deepcopy(self.managed)

    def apply(self, managed, retiring=(), starting=(), before_apply=None):
        if before_apply:
            before_apply({"resource_changes": deepcopy(self.resource_changes), "output_changes": deepcopy(self.output_changes)})
        # The same authorization rules as cloud.fence_change, at intent level.
        for identity, value in managed.items():
            if value["running"] and not self.managed.get(identity, {}).get("running") and identity not in starting:
                raise RuntimeError("plan would start an Environment node pool without a current explicit start authorization")
            if value["published"] and not (value["running"] and value["address"]):
                raise AssertionError("DNS requires a running Environment address")
        if set(self.managed) - set(managed) - set(retiring):
            raise RuntimeError("plan would delete a node pool without explicit retirement")
        self.plans.append((deepcopy(managed), set(retiring)))
        self.managed = deepcopy(managed)
        self.brokers = deepcopy(self.broker_configuration(managed))
        self.applied = True

    def outputs(self):
        return {
            key: dict(
                namespace="aidash-" + key,
                hostname=f"{key}.aidash.run",
                runtime_secret=key,
                server_service_account=f"{key}-server@{PROJECT}.iam.gserviceaccount.com",
                worker_service_account=f"{key}-worker@{PROJECT}.iam.gserviceaccount.com",
                provider_credentials=None,
                gcip=deepcopy(self.gcip),
            )
            for key in self.managed
        }

    def shared_outputs(self):
        return {
            "cluster": dict(
                name="aidash", location="us-central1-a", project_id=PROJECT,
                workload_pool=PROJECT + ".svc.id.goog", pod_cidr="10.44.0.0/14",
            ) if self.applied else None,
            "preview_tls_volume_handle": f"projects/{PROJECT}/zones/us-central1-a/disks/aidash-preview-tls",
        }


class ReconcileTests(unittest.TestCase):
    def setUp(self):
        self.store = MemoryStore()
        self.cloud = CloudFixture()
        self.config = deepcopy(CONFIG)
        self.sequence = 0
        self.calls = []
        self.kube = FakeCluster(self.calls)
        self.closed = set()
        self.retirement_failure = False
        self.context = ExitStack()
        self.addCleanup(self.context.close)
        replacements = {
            "Terraform": self.terraform,
            "ci_success": lambda *args: None,
            "provision_secret": lambda *args: deepcopy(RUNTIME),
            "public_health": lambda *args: None,
            "github": self.github,
            "retire_provider_credentials": self.retire_credentials,
        }
        for key, value in replacements.items():
            self.context.enter_context(patch.object(controller, key, value))
        self.context.enter_context(patch.object(controller.kube, "connect", lambda config, cluster, directory: self.kube))
        self.context.enter_context(patch.object(controller.kube.time, "sleep"))
        self.context.enter_context(
            patch.object(controller.time, "time", return_value=10000)
        )

    @property
    def busy(self):
        return bool(self.kube.busy)

    @busy.setter
    def busy(self, value):
        self.kube.busy = {"test", "develop", "pr-1", "pr-2"} if value is True else set(value or ())

    @property
    def idle(self):
        return bool(self.kube.idle)

    @idle.setter
    def idle(self, value):
        self.kube.idle = {"test", "develop", "pr-1", "pr-2"} if value else set()

    def github(self, path):
        identity = "pr-" + path.rsplit("/", 1)[1]
        return dict(
            state="closed" if identity in self.closed else "open",
            base={"ref": "main"},
            head={"sha": self.store.state["environments"][identity]["sha"]},
        )

    def terraform(self, root, configuration):
        self.cloud.configuration = configuration
        return self.cloud

    def request(self, identity="test", action="create", **extra):
        self.sequence += 1
        request = dict(
            environment=identity,
            action=action,
            sequence=self.sequence,
            sha=SHA,
            source_ref="pr/" + identity[3:] if identity.startswith("pr-") else "main",
            source_repo=CONFIG["repository"],
        )
        request.update(extra)
        self.store.state = transition(self.store.state, request, "c" * 12, 0)[0]
        entry = self.store.state["environments"][identity]
        if action not in {"stop", "destroy"}:
            entry["release"] = {"source_sha": entry["sha"], "images": deepcopy(IMAGES)}
        return entry

    def retire_credentials(self, config, identity):
        if config.get("byok_project_id"):
            self.calls.append(("retire_secrets", identity))
            if self.retirement_failure:
                raise RuntimeError("retirement inventory unavailable")

    def reconcile(self, byok=False):
        config = dict(self.config, byok_project_id="fixture-byok") if byok else self.config
        controller.reconcile(config, self.store)

    def deployed(self, identity):
        return ("helm", identity, "app") in self.calls

    def stopped(self, identity):
        return ("scale", identity, "runner", 0) in self.calls

    def managed_outputs(self, provider):
        outputs = self.cloud.outputs

        def values():
            result = outputs()
            for identity, output in result.items():
                output["provider_credentials"] = provider(identity)
            return result

        return patch.object(self.cloud, "outputs", side_effect=values)

    def test_create_installs_dependencies_migrates_with_writers_stopped_then_publishes(self):
        self.request()
        applies = []
        apply = self.cloud.apply

        def recorded_apply(managed, **kwargs):
            applies.append((deepcopy(managed["test"]), len(self.calls)))
            apply(managed, **kwargs)

        with patch.object(self.cloud, "apply", side_effect=recorded_apply):
            self.reconcile()
        entry = self.store.state["environments"]["test"]
        self.assertEqual(entry["status"], "ready")
        events = [call for call in self.calls if call[0] in {"helm", "migrate", "admission"} or call[:3] == ("scale", "test", "server")]
        self.assertEqual(events, [
            ("helm", "test", "env"), ("helm", "test", "app"), ("migrate", "test"),
            ("scale", "test", "server", 1), ("admission", "test", "open"),
        ])
        # Node pool first without DNS; DNS only with the edge address, before admission opens.
        self.assertEqual([(value["running"], value["published"], value["address"]) for value, _ in applies], [
            (True, False, None), (True, True, "203.0.113.1"),
        ])
        self.assertLessEqual(applies[1][1], self.calls.index(("admission", "test", "open")))
        app = self.kube.releases[("aidash-test", "app")]
        self.assertEqual(app["server"]["replicas"], 0)
        self.assertEqual(app["release"]["sourceSha"], SHA)
        self.assertEqual(app["image"], {"repository": IMAGES["app"].split("@")[0], "digest": "sha256:" + "d" * 64})
        self.assertEqual(app["trustedProxy"], {"cidrs": ["10.44.0.0/14"]})
        self.assertEqual(app["execution"]["runner"]["image"], IMAGES["control"])
        env = self.kube.releases[("aidash-test", "env")]
        self.assertEqual(env["edge"]["existingTlsClaim"], "")
        self.assertEqual(env["activity"]["collectorImage"], IMAGES["control"])
        runtime = self.kube.secret("aidash-test", "app-runtime")
        tokens = self.kube.secret("aidash-test", "aidash-identity")
        self.assertEqual(runtime["DATABASE_URL"], f"postgres://aidash:{tokens['database']}@env-environment-postgres:5432/aidash_a")
        self.assertEqual(runtime["AIDASH_OIDC_PUBLIC_ORIGIN"], "https://test.aidash.run")
        self.assertNotIn(tokens["database"], json.dumps(list(self.kube.releases.values())))
        self.assertEqual(self.kube.secret("aidash-test", "env-postgres")["POSTGRES_PASSWORD"], tokens["database"])
        # Retained identity is generated once and never regenerated.
        self.request(action="resume", sha="b" * 40)
        self.reconcile()
        self.assertEqual(self.kube.secret("aidash-test", "aidash-identity"), tokens)

    def test_ready_environment_reconciles_broker_enable_rotation_disable_and_removal(self):
        self.request()
        self.reconcile()
        self.assertEqual(self.store.state["environments"]["test"]["status"], "ready")
        broker = {"enabled": True, "image": "sha256:" + "a" * 64}
        for desired in [
            broker,
            dict(broker, image="sha256:" + "b" * 64),
            dict(broker, enabled=False),
            None,
        ]:
            with self.subTest(desired=desired):
                previously_enabled = self.cloud.brokers.get("test", {}).get("enabled", False)
                self.config["credential_brokers"] = {"retired": broker}
                expected = {} if desired is None else {"test": desired}
                self.config["credential_brokers"].update(expected)
                self.cloud.plans.clear()
                self.calls.clear()
                self.reconcile()
                self.assertEqual(len(self.cloud.plans), 1)
                self.assertEqual(self.cloud.brokers, expected)
                self.assertTrue(self.cloud.managed["test"]["running"])
                self.assertFalse(self.deployed("test") or self.stopped("test"))
                drain_required = previously_enabled or bool((desired or {}).get("enabled"))
                self.assertEqual(("observe", "test") in self.calls, drain_required)
                self.assertEqual(("admission", "test", "open") in self.calls, drain_required)
                self.cloud.plans.clear()
                self.reconcile()
                self.assertEqual(self.cloud.plans, [])

    def test_live_broker_disable_removal_and_rotation_wait_before_any_apply(self):
        broker = {"enabled": True, "image": "initial"}
        self.config["credential_brokers"] = {"test": broker}
        self.request()
        self.reconcile()
        for desired in [{"test": dict(broker, enabled=False)}, {}, {"test": dict(broker, image="rotated")}]:
            with self.subTest(desired=desired):
                self.config["credential_brokers"] = {"test": broker}
                self.reconcile()
                self.config["credential_brokers"] = desired
                self.cloud.plans.clear()
                self.calls.clear()
                self.busy = True
                self.reconcile()
                self.assertEqual(self.cloud.plans, [])
                self.assertEqual(self.cloud.brokers, {"test": broker})
                self.assertEqual(self.store.state["environments"]["test"]["status"], "waiting_for_active_work")
                self.assertEqual(self.calls, seal("test") + unseal("test"))
                self.busy = False
                self.calls.clear()
                apply = self.cloud.apply
                def recorded_apply(managed, apply=apply, **kwargs):
                    self.calls.append(("broker_apply", "test"))
                    apply(managed, **kwargs)
                with patch.object(self.cloud, "apply", side_effect=recorded_apply):
                    self.reconcile()
                self.assertLess(self.calls.index(("observe", "test")), self.calls.index(("broker_apply", "test")))
                self.assertIn(("admission", "test", "open"), self.calls)
                self.assertFalse(self.deployed("test") or self.stopped("test"))
                self.assertEqual(self.cloud.brokers, desired)
                self.cloud.plans.clear()
                self.calls.clear()
                self.reconcile()
                self.assertEqual(self.cloud.plans, [])

    def test_project_change_refuses_before_cleanup_or_apply(self):
        self.request()
        self.reconcile(byok=True)
        self.request(action="destroy")
        self.calls.clear()
        before = deepcopy(self.store.state)
        plans = deepcopy(self.cloud.plans)
        self.cloud.configuration = dict(CONFIG, byok_project_id="")
        applied = {"outputs": {
            "managed_configuration": {"value": deepcopy(self.cloud.managed)},
            "byok_project_id": {"value": "fixture-byok"},
            "environments": {"value": {"test": {"byok_project_id": "fixture-byok"}}},
        }}
        read = self.store.read
        with (
            patch.object(self.store, "read", side_effect=lambda key:
                (deepcopy(applied), "1") if key.endswith("default.tfstate") else read(key)),
            patch.object(self.cloud, "configuration_in_state", side_effect=lambda store:
                NativeTerraform.configuration_in_state(self.cloud, store)),
        ):
            for configured in ["", "replacement-byok"]:
                with self.subTest(configured=configured):
                    self.cloud.configuration["byok_project_id"] = configured
                    with self.assertRaisesRegex(RuntimeError, "Restore the applied BYOK project"):
                        controller.reconcile(self.cloud.configuration, self.store)
        self.assertEqual(self.calls, [])
        self.assertEqual(self.cloud.plans, plans)
        self.assertEqual(self.store.state, before)

    def broker_pair(self):
        brokers = {identity: {"enabled": True, "image": "initial"} for identity in ["develop", "test"]}
        self.config["develop_branch"] = "develop/0.1.0"
        self.config["credential_brokers"] = brokers
        for identity in brokers:
            self.request(identity, source_ref="develop/0.1.0" if identity == "develop" else "main")
            self.reconcile()
        return brokers

    def test_blocked_multi_environment_broker_change_restores_sealed_environments(self):
        brokers = self.broker_pair()
        for busy in brokers:
            with self.subTest(busy=busy):
                self.config["credential_brokers"] = brokers
                self.reconcile()
                self.config["credential_brokers"] = {}
                self.cloud.plans.clear()
                self.calls.clear()
                self.busy = {busy}
                self.reconcile()
                self.busy = False
                self.assertEqual(self.cloud.plans, [])
                self.assertEqual(self.cloud.brokers, brokers)
                idle = next(identity for identity in brokers if identity != busy)
                expected = []
                for identity in ["develop", "test"]:
                    expected += seal(identity) + (unseal(identity) if identity == busy else [])
                self.assertEqual(self.calls, expected + unseal(idle))
                self.assertEqual(self.store.state["environments"][busy]["status"], "waiting_for_active_work")

    def test_broker_preflight_failure_restores_prior_seals(self):
        brokers = self.broker_pair()
        self.config["credential_brokers"] = {}
        self.cloud.plans.clear()
        self.calls.clear()
        self.kube.fail(("observe", "test"), RuntimeError("seal unavailable"))
        with self.assertRaisesRegex(RuntimeError, "seal unavailable"):
            self.reconcile()
        self.assertEqual(self.calls, seal("develop") + seal("test")[:-1] + unseal("test") + unseal("develop"))
        self.assertEqual(self.cloud.plans, [])
        self.assertEqual(self.cloud.brokers, brokers)

    def test_broker_apply_failure_restores_all_presealed_environments_and_can_retry(self):
        brokers = self.broker_pair()
        for desired in [{key: dict(value, enabled=False) for key, value in brokers.items()}, {}, {key: dict(value, image="rotated") for key, value in brokers.items()}]:
            with self.subTest(desired=desired):
                self.config["credential_brokers"] = brokers
                self.reconcile()
                self.config["credential_brokers"] = desired
                self.calls.clear()
                managed = deepcopy(self.cloud.managed)
                def failed_apply(*args, **kwargs):
                    self.calls.append(("apply", "all"))
                    raise RuntimeError("plan/apply unavailable")
                with patch.object(self.cloud, "apply", side_effect=failed_apply), self.assertRaisesRegex(RuntimeError, "plan/apply unavailable"):
                    self.reconcile()
                self.assertEqual(self.calls, seal("develop") + seal("test") + [("apply", "all")] + unseal("develop") + unseal("test"))
                self.assertEqual(self.cloud.managed, managed)
                self.assertEqual(self.cloud.brokers, brokers)
                self.calls.clear()
                self.reconcile()
                self.assertEqual(self.cloud.brokers, desired)
                self.assertFalse(any(self.deployed(identity) or self.stopped(identity) for identity in brokers))

    def test_failed_broker_apply_restoration_still_attempts_every_environment(self):
        self.broker_pair()
        self.config["credential_brokers"] = {}
        self.calls.clear()
        self.kube.fail(("admission", "develop", "open"), RuntimeError("unseal unavailable"))
        with patch.object(self.cloud, "apply", side_effect=RuntimeError("apply unavailable")), self.assertRaisesRegex(RuntimeError, "admission restoration failed") as failure:
            self.reconcile()
        self.assertEqual(self.calls, seal("develop") + seal("test") + unseal("develop") + unseal("test"))
        self.assertIn("apply unavailable", str(failure.exception.__context__))

    def test_broker_deadline_restores_presealed_environments_with_a_fresh_cleanup_budget(self):
        from cloud import DEADLINE
        brokers = self.broker_pair()
        for stage in ["preflight", "apply"]:
            with self.subTest(stage=stage):
                self.config["credential_brokers"] = {}
                self.calls.clear()
                def expire(namespace, stage=stage):
                    if stage == "preflight" and namespace == "aidash-test":
                        DEADLINE.set(0)
                        controller.bounded_timeout(1)
                def deadline_apply(*args, **kwargs):
                    self.calls.append(("apply", "all"))
                    DEADLINE.set(0)
                    controller.bounded_timeout(1)
                self.kube.on_observe = expire
                with controller.operation_budget(60), patch.object(self.cloud, "apply", side_effect=deadline_apply), self.assertRaises(controller.OperationDeadline):
                    self.reconcile()
                self.kube.on_observe = None
                expected = seal("develop")
                if stage == "apply":
                    expected += seal("test") + [("apply", "all")] + unseal("develop") + unseal("test")
                else:
                    # The expired seal cannot even start its own restore command.
                    expected += seal("test")[:4] + unseal("develop")
                self.assertEqual(self.calls, expected)
                self.assertEqual(self.cloud.brokers, brokers)
                self.assertFalse(self.store.locked)

    def test_broker_restoration_attempts_every_environment_after_failure_or_deadline(self):
        terraform = type("Fixture", (), {"cluster": lambda _: self.kube})()
        for error in [RuntimeError("unseal unavailable"), controller.OperationDeadline("fixture cleanup deadline")]:
            with self.subTest(error=type(error).__name__):
                attempts = []
                def restore(cluster, identity, error=error, attempts=attempts):
                    attempts.append(identity)
                    if identity == "develop":
                        raise error
                with patch.object(controller.kube, "unseal", side_effect=restore), self.assertRaisesRegex(RuntimeError, "admission restoration failed"):
                    controller.restore_broker_admission(terraform, {"develop", "test"})
                self.assertEqual(attempts, ["develop", "test"])

    def test_byok_retirement_freezes_writers_without_starting_a_stopped_environment(self):
        for running in [True, False]:
            with self.subTest(running=running):
                self.request()
                self.reconcile()
                if not running:
                    self.request(action="stop")
                    self.reconcile()
                self.request(action="destroy")
                self.calls.clear()
                plans = len(self.cloud.plans)
                apply = self.cloud.apply

                def recorded_apply(managed, apply=apply, **kwargs):
                    self.calls.append(("apply", bool(kwargs.get("retiring"))))
                    if kwargs.get("retiring") or "test" not in managed:
                        self.assertIn(("retire_secrets", "test"), self.calls)
                    apply(managed, **kwargs)

                with patch.object(self.cloud, "apply", side_effect=recorded_apply):
                    self.reconcile(byok=True)
                self.assertNotIn("test", self.cloud.managed)
                self.assertEqual(any(value["test"]["running"] for value, _ in self.cloud.plans[plans:] if "test" in value), running)
                self.assertNotIn(("observe", "test"), self.calls)
                self.assertNotIn(("admission", "test", "open"), self.calls)
                self.assertFalse(self.deployed("test"))
                freeze = self.calls.index(("scale", "test", "server", 0))
                self.assertLess(freeze, self.calls.index(("retire_secrets", "test")))
                self.assertLess(self.calls.index(("retire_secrets", "test")), self.calls.index(("apply", True)))
                self.assertLess(self.calls.index(("delete_namespace", "aidash-test")), self.calls.index(("apply", True)))

    def test_byok_cleanup_failure_preserves_identity_and_disks_then_retries(self):
        self.request()
        self.reconcile()
        self.request(action="destroy")
        self.calls.clear()
        self.retirement_failure = True
        plans = len(self.cloud.plans)
        with self.assertRaises(RuntimeError):
            self.reconcile(byok=True)
        self.assertIn("test", self.cloud.managed)
        self.assertEqual(self.store.state["environments"]["test"]["status"], "failed")
        self.assertFalse(any(retiring for _, retiring in self.cloud.plans[plans:]))
        self.assertLess(self.calls.index(("admission", "test", "close")), self.calls.index(("retire_secrets", "test")))
        self.assertNotIn(("admission", "test", "open"), self.calls)
        self.assertFalse(any(call[0] in {"delete_namespace", "delete_disk"} for call in self.calls))
        self.retirement_failure = False
        self.reconcile(byok=True)
        self.assertNotIn("test", self.cloud.managed)

    def test_partial_byok_retirement_still_cleans_without_a_managed_environment(self):
        self.request()
        self.reconcile()
        self.request(action="destroy")
        del self.cloud.managed["test"]
        self.calls.clear()
        self.reconcile(byok=True)
        self.assertEqual(self.calls, [("retire_secrets", "test")])
        self.assertEqual(self.store.state["environments"]["test"]["status"], "destroyed")

    def test_node_pool_starts_only_with_an_explicit_start_authorization(self):
        self.request()
        self.reconcile()
        self.request(action="resume")
        self.reconcile()
        self.assertFalse(self.store.state["environments"]["test"]["start_pending"])
        self.request(action="stop")
        self.reconcile()
        entry = self.store.state["environments"]["test"]
        # A running intent without a pending start (e.g. a stale retry) never starts the pool.
        entry.update(desired="running", sha="b" * 40, release={"source_sha": "b" * 40, "images": deepcopy(IMAGES)})
        self.cloud.plans.clear()
        self.calls.clear()
        self.reconcile()
        self.assertEqual(self.store.state["environments"]["test"]["status"], "interrupted")
        self.assertEqual(self.store.state["environments"]["test"]["desired"], "stopped")
        self.assertEqual(self.cloud.plans, [])
        self.assertFalse(self.cloud.managed["test"]["running"])
        self.assertFalse(self.deployed("test"))

    def test_provider_descriptor_change_waits_for_work_then_reloads_once_without_restarting(self):
        self.request()
        self.reconcile()
        expected = descriptor("test")
        with self.managed_outputs(lambda identity: expected if self.cloud.brokers.get(identity, {}).get("enabled") else None):
            for enabled in [True, False]:
                self.config["credential_brokers"] = {"test": {"enabled": enabled, "image": "fixture"}}
                self.calls.clear()
                self.busy = True
                self.reconcile()
                self.assertEqual(self.store.state["environments"]["test"]["status"], "waiting_for_active_work")
                self.assertFalse(self.deployed("test") or self.stopped("test"))
                self.busy = False
                self.calls.clear()
                self.reconcile()
                entry = self.store.state["environments"]["test"]
                self.assertEqual(entry["status"], "ready")
                self.assertEqual(entry["provider_credentials"], expected if enabled else None)
                self.assertEqual([call for call in self.calls if call[0] in {"helm", "migrate"}], [("helm", "test", "app")])
                app = self.kube.releases[("aidash-test", "app")]
                self.assertEqual(app["providerCredentials"]["settings"], expected if enabled else {"fingerprint_key": None, "store": None, "broker": None})
                self.assertEqual(app["release"]["sourceSha"], SHA)
                self.assertFalse(self.stopped("test"))
                self.calls.clear()
                self.cloud.plans.clear()
                self.reconcile()
                self.assertEqual(self.cloud.plans, [])
                self.assertFalse(self.deployed("test"))

    def test_gcip_changes_refresh_all_running_and_retained_stopped_environments(self):
        self.request()
        self.reconcile()
        self.request("pr-1")
        self.reconcile()
        self.request("develop", source_ref="develop/0.1.0")
        self.reconcile()
        self.request(action="stop")
        self.reconcile()
        pending = self.request(
            "develop", "update", sha="b" * 40, source_ref="develop/0.1.0"
        )
        pending["release"] = None
        config = dict(CONFIG, gcip_tenants={"company": {"tenant": "old"}})
        secrets = {}
        original_apply = self.cloud.apply

        def apply(*args, **kwargs):
            # Every running Environment is fenced before the shared input is
            # applied, including one processed later in the environment loop.
            for identity in ("pr-1", "develop"):
                self.assertIn(("scale", identity, "runner", 0), self.calls)
            original_apply(*args, **kwargs)
            self.cloud.gcip = {
                "tenant_ids": ["pool"] if config["gcip_tenants"] else [],
                "tenant_bindings": {
                    "pool": config["gcip_tenants"]["company"]["tenant"]
                } if config["gcip_tenants"] else {},
            }

        def provision(config, output, kind):
            secrets[output["runtime_secret"]] = deepcopy(output["gcip"])
            return deepcopy(RUNTIME)

        with (
            patch.object(self.cloud, "apply", apply),
            patch.object(controller, "provision_secret", provision),
            patch.object(controller, "reconcile_environment") as iam,
            patch.dict(os.environ, {"AIDASH_GCIP_IDP_SECRETS": "{}"}),
        ):
            for change in ("enable", "remap", "rotate", "remove"):
                with self.subTest(change=change):
                    if change == "remap":
                        config["gcip_tenants"]["company"]["tenant"] = "new"
                    elif change == "rotate":
                        os.environ["AIDASH_GCIP_IDP_SECRETS"] = '{"company/oidc.sso":"rotated-private"}'
                    elif change == "remove":
                        config["gcip_tenants"] = {}
                    self.calls.clear()
                    secrets.clear()
                    controller.reconcile(config, self.store)
                    self.assertEqual(set(secrets), {"test", "pr-1", "develop"})
                    for identity in ("pr-1", "develop"):
                        self.assertIn(("helm", identity, "app"), self.calls)
                        self.assertIn(("admission", identity, "open"), self.calls)
                        self.assertEqual(self.kube.releases[("aidash-" + identity, "app")]["release"]["sourceSha"], SHA)
                    self.assertFalse(self.deployed("test"))
                    self.assertFalse(self.cloud.managed["test"]["running"])
                    self.assertEqual(self.cloud.managed["develop"]["release_sha"], SHA)
                    self.assertEqual(
                        self.store.state["environments"]["develop"]["status"],
                        "awaiting_build",
                    )
                    revision = controller.gcip_revision(config)
                    for entry in self.store.state["environments"].values():
                        self.assertEqual(entry["gcip_revision"], revision)
                        self.assertFalse(entry["gcip_pending"])
                    self.assertNotIn("rotated-private", json.dumps(self.store.state))
                    self.assertEqual(secrets["pr-1"], self.cloud.gcip)
                    self.assertTrue(iam.called)
                    self.calls.clear()
                    secrets.clear()
                    plans = len(self.cloud.plans)
                    controller.reconcile(config, self.store)
                    self.assertEqual(len(self.cloud.plans), plans)
                    self.assertFalse(secrets)
                    self.assertFalse(any(call[0] == "helm" for call in self.calls))

    def fail_reload(self, identity):
        reload = controller.kube.reload

        def failing(cluster, name, *args):
            self.calls.append(("reload", name))
            if name == identity:
                raise RuntimeError("fixture reload failure")
            return reload(cluster, name, *args)

        return patch.object(controller.kube, "reload", failing)

    def test_failed_gcip_refresh_stays_gated_and_retries_without_restarting_successes(self):
        self.request()
        self.reconcile()
        self.request("pr-1")
        self.reconcile()
        config = dict(CONFIG, gcip_tenants={"company": {"tenant": "new"}})
        self.calls.clear()
        with self.fail_reload("test"):
            with self.assertRaisesRegex(RuntimeError, "Reconciliation incomplete"):
                controller.reconcile(config, self.store)
        self.assertNotIn(("admission", "test", "open"), self.calls)
        failed = [call for call in self.calls if call[1] == "test"]
        self.assertEqual(failed[-1], ("scale", "test", "runner", 0))
        self.assertEqual(failed.count(("scale", "test", "runner", 0)), 2)
        self.assertIn(("admission", "pr-1", "open"), self.calls)
        self.assertTrue(self.store.state["environments"]["test"]["gcip_pending"])
        self.assertFalse(self.store.state["environments"]["pr-1"]["gcip_pending"])
        self.calls.clear()
        controller.reconcile(config, self.store)
        self.assertIn(("helm", "test", "app"), self.calls)
        self.assertNotIn(("helm", "pr-1", "app"), self.calls)
        self.assertFalse(self.store.state["environments"]["test"]["gcip_pending"])

    def test_unconfirmed_gcip_quiescence_aborts_before_shared_apply(self):
        self.request()
        self.reconcile()
        config = dict(CONFIG, gcip_tenants={"company": {"tenant": "new"}})
        plans = len(self.cloud.plans)
        with patch.object(controller.kube, "quiesce", return_value=False):
            with self.assertRaisesRegex(controller.Refused, "quiescence was not confirmed"):
                controller.reconcile(config, self.store)
        self.assertEqual(len(self.cloud.plans), plans)
        self.assertTrue(self.store.state["environments"]["test"]["gcip_pending"])

    def test_gcip_change_fences_a_running_unpublished_failed_deployment(self):
        self.request()
        self.reconcile()
        self.cloud.managed["test"].update(published=False, address=None)
        entry = self.store.state["environments"]["test"]
        entry["failed_deployment_generation"] = entry["generation"]
        self.calls.clear()
        original = self.cloud.apply

        def apply(*args, **kwargs):
            self.assertIn(("scale", "test", "runner", 0), self.calls)
            original(*args, **kwargs)

        with patch.object(self.cloud, "apply", apply):
            controller.reconcile(dict(CONFIG, gcip_tenants={"company": {"tenant": "new"}}), self.store)
        self.assertNotIn(("admission", "test", "open"), self.calls)
        self.assertFalse(self.cloud.managed["test"]["published"])

    def test_unrelated_apply_refreshes_generated_gcip_id_drift(self):
        self.exercise_gcip_output_drift(
            [{"type": "google_identity_platform_tenant", "change": {"actions": ["delete", "create"]}}], {}
        )

    def test_output_only_gcip_id_drift_is_fenced_before_apply(self):
        self.exercise_gcip_output_drift([], {"environments": {
            "before": {"test": {"gcip": {"tenant_ids": ["old-pool"], "tenant_bindings": {"old-pool": "acme"}}}},
            "after": {"test": {"gcip": {"tenant_ids": ["new-pool"], "tenant_bindings": {"new-pool": "acme"}}}},
        }})

    def test_first_gcip_creation_does_not_query_outputs_before_apply(self):
        self.request()
        original = self.cloud.outputs

        def outputs():
            self.assertTrue(self.cloud.managed, "no environments output exists before the first apply")
            return original()

        with patch.object(self.cloud, "outputs", outputs):
            controller.reconcile(dict(CONFIG, gcip_tenants={"company": {"tenant": "acme"}}), self.store)
        self.assertEqual(self.store.state["environments"]["test"]["status"], "ready")

    def exercise_gcip_output_drift(self, resource_changes, output_changes):
        self.context.enter_context(patch.object(controller, "reconcile_environment"))
        config = dict(CONFIG, gcip_tenants={"company": {"tenant": "acme"}})
        self.cloud.gcip = {"tenant_ids": ["old-pool"], "tenant_bindings": {"old-pool": "acme"}}
        self.request()
        controller.reconcile(config, self.store)
        self.request("develop", source_ref="develop/0.1.0")
        controller.reconcile(config, self.store)
        self.request("develop", "stop")
        controller.reconcile(config, self.store)
        self.request("pr-1")
        self.calls.clear()
        original = self.cloud.apply
        secrets = {}
        self.cloud.resource_changes = resource_changes
        self.cloud.output_changes = output_changes

        def apply(*args, **kwargs):
            original(*args, **kwargs)
            self.assertIn(("scale", "test", "runner", 0), self.calls)
            self.cloud.gcip = {"tenant_ids": ["new-pool"], "tenant_bindings": {"new-pool": "acme"}}
            self.cloud.resource_changes = []
            self.cloud.output_changes = {}

        def provision(config, output, kind):
            secrets[output["runtime_secret"]] = deepcopy(output["gcip"])
            return deepcopy(RUNTIME)

        with patch.object(self.cloud, "apply", apply), patch.object(controller, "provision_secret", provision), patch.object(controller, "reconcile_environment") as iam:
            controller.reconcile(config, self.store)
        self.assertEqual(secrets["test"]["tenant_ids"], ["new-pool"])
        self.assertEqual(secrets["develop"]["tenant_ids"], ["new-pool"])
        self.assertIn(("helm", "test", "app"), self.calls)
        self.assertNotIn(("helm", "develop", "app"), self.calls)
        self.assertFalse(self.cloud.managed["develop"]["running"])
        self.assertTrue(iam.called)
        self.calls.clear()
        plans = len(self.cloud.plans)
        controller.reconcile(config, self.store)
        self.assertEqual(len(self.cloud.plans), plans)
        self.assertFalse(any(call[0] == "helm" or call[2:3] == ("runner",) for call in self.calls))

    def test_gcip_refresh_failure_cannot_unseal_through_an_ordinary_stop(self):
        self.request()
        self.reconcile()
        self.request(action="stop")
        config = dict(CONFIG, gcip_tenants={"company": {"tenant": "new"}})
        with self.fail_reload("test"):
            self.calls.clear()
            with self.assertRaisesRegex(RuntimeError, "Reconciliation incomplete"):
                controller.reconcile(config, self.store)
            self.assertNotIn(("observe", "test"), self.calls)
            self.assertNotIn(("admission", "test", "open"), self.calls)
            self.assertTrue(self.cloud.managed["test"]["running"])
            self.assertTrue(self.store.state["environments"]["test"]["gcip_quiesced"])
            self.request(action="stop", force=True)
            self.calls.clear()
            with self.assertRaisesRegex(RuntimeError, "Reconciliation incomplete"):
                controller.reconcile(config, self.store)
            self.assertIn(("scale", "test", "postgres", 0), self.calls)
            self.assertNotIn(("admission", "test", "open"), self.calls)
            self.assertFalse(self.cloud.managed["test"]["running"])

    def test_failed_gcip_refresh_does_not_block_closed_pr_retirement(self):
        self.request()
        self.reconcile()
        self.request("pr-1")
        self.reconcile()
        self.closed.add("pr-1")
        self.calls.clear()
        config = dict(CONFIG, gcip_tenants={"company": {"tenant": "new"}})
        with self.fail_reload("test"):
            with self.assertRaisesRegex(RuntimeError, "Reconciliation incomplete"):
                controller.reconcile(config, self.store)
        self.assertNotIn("pr-1", self.cloud.managed)
        self.assertEqual(self.store.state["environments"]["pr-1"]["status"], "destroyed")
        self.assertNotIn(("reload", "pr-1"), self.calls)
        self.assertNotIn(("admission", "pr-1", "open"), self.calls)
        self.assertTrue(self.store.state["environments"]["test"]["gcip_pending"])

    def test_stop_and_resume_keep_claims_and_resume_reauthorizes_the_pool(self):
        self.request()
        self.reconcile()
        tokens = self.kube.secret("aidash-test", "aidash-identity")
        self.request(action="stop")
        self.reconcile()
        self.assertFalse(self.cloud.managed["test"]["running"])
        self.assertIsNone(self.kube.get("service", "env-environment-edge", "aidash-test"))
        self.assertIsNotNone(self.kube.get("persistentvolumeclaim", "memory-recovery", "aidash-test"))
        self.assertTrue(self.kube.objects[("cronjob", "aidash-test", "env-environment-activity")]["spec"]["suspend"])
        self.calls.clear()
        self.request(action="resume")
        self.reconcile()
        self.assertEqual(self.store.state["environments"]["test"]["status"], "ready")
        self.assertIn(("suspend", "test", False), self.calls)
        self.assertEqual(self.cloud.managed["test"]["address"], "203.0.113.2")
        self.assertEqual(self.kube.secret("aidash-test", "aidash-identity"), tokens)
        self.request(action="resume", mode="normal")
        self.reconcile()
        self.assertFalse(self.cloud.managed["test"]["spot"])
        self.assertEqual(self.calls.count(("helm", "test", "app")), 2)

    def test_active_work_defers_update_and_then_installs_latest_source(self):
        self.request("pr-1")
        self.reconcile()
        self.request("pr-1", "update", sha="b" * 40)
        self.busy = True
        self.reconcile()
        self.assertEqual(self.cloud.managed["pr-1"]["release_sha"], SHA)
        self.assertEqual(
            self.store.state["environments"]["pr-1"]["status"],
            "waiting_for_active_work",
        )
        self.request("pr-1", "update", sha="c" * 40)
        self.busy = False
        self.reconcile()
        self.assertEqual(self.cloud.managed["pr-1"]["release_sha"], "c" * 40)
        self.assertEqual(self.calls.count(("helm", "pr-1", "app")), 2)
        self.assertEqual(self.kube.get("deployment", "app-aidash-server", "aidash-pr-1")["metadata"]["annotations"]["aidash.run/source-sha"], "c" * 40)

    def test_fresh_seal_observation_must_be_idle_or_service_is_restored(self):
        self.request()
        self.reconcile()
        self.request(action="resume", sha="b" * 40)
        # "foreign": an idle snapshot that a concurrent minute CronJob run published
        # after the seal's own Job, from evidence gathered before the drain.
        for mode in ["busy", "stale", "failed", "error", "foreign"]:
            with self.subTest(mode=mode):
                self.calls.clear()
                self.kube.script = [mode]
                self.reconcile()
                entry = self.store.state["environments"]["test"]
                self.assertEqual(entry["status"], "waiting_for_active_work")
                self.assertEqual(self.cloud.managed["test"]["release_sha"], SHA)
                self.assertEqual(self.calls[-3:], unseal("test"))
                self.assertFalse(self.stopped("test") or self.deployed("test"))
        self.calls.clear()
        self.reconcile()
        self.assertEqual(self.cloud.managed["test"]["release_sha"], "b" * 40)

    def test_runner_and_dependencies_stop_only_after_the_post_drain_observation(self):
        self.request()
        self.reconcile()
        self.request(action="stop")
        self.calls.clear()
        apply = self.cloud.apply

        def recorded_apply(managed, **kwargs):
            self.calls.append(("apply", managed["test"]["published"], managed["test"]["running"]))
            apply(managed, **kwargs)

        with patch.object(self.cloud, "apply", side_effect=recorded_apply):
            self.reconcile()
        drained = self.calls.index(("scale", "test", "worker", 0))
        observed = self.calls.index(("observe", "test"))
        self.assertLess(self.calls.index(("admission", "test", "close")), drained)
        self.assertLess(drained, observed)
        for name in ("runner", "edge", "postgres", "nats"):
            self.assertLess(observed, self.calls.index(("scale", "test", name, 0)))
        self.assertLess(observed, self.calls.index(("apply", False, True)))
        self.assertLess(self.calls.index(("delete_lb", "test")), self.calls.index(("apply", False, False)))
        self.assertEqual(self.store.state["environments"]["test"]["status"], "stopped")

    def test_preview_tls_rebinding_waits_for_the_previous_attachment(self):
        self.request("pr-1")
        self.reconcile()
        self.assertEqual(self.kube.releases[("aidash-pr-1", "env")]["edge"]["existingTlsClaim"], "preview-tls")
        volume = self.kube.get("persistentvolume", "aidash-preview-tls")
        self.assertEqual(volume["spec"]["claimRef"]["namespace"], "aidash-pr-1")
        self.assertEqual(volume["spec"]["csi"]["volumeHandle"], f"projects/{PROJECT}/zones/us-central1-a/disks/aidash-preview-tls")
        self.request("pr-1", "stop")
        self.reconcile()
        self.kube.attached["aidash-preview-tls"] = 1
        self.request("pr-2")
        plans = len(self.cloud.plans)
        self.calls.clear()
        self.reconcile()
        self.assertEqual(self.store.state["environments"]["pr-2"]["status"], "waiting_for_preview_tls")
        self.assertEqual(len(self.cloud.plans), plans)
        self.assertEqual(self.kube.get("persistentvolume", "aidash-preview-tls")["spec"]["claimRef"]["namespace"], "aidash-pr-1")
        self.calls.clear()
        self.reconcile()
        self.assertEqual(self.store.state["environments"]["pr-2"]["status"], "ready")
        released = self.calls.index(("delete", "pvc", "pr-1", "preview-tls"))
        self.assertLess(released, self.calls.index(("patch_pv", "aidash-preview-tls", "aidash-pr-2")))
        self.assertLess(released, self.calls.index(("helm", "pr-2", "env")))
        self.assertEqual(self.kube.get("persistentvolume", "aidash-preview-tls")["spec"]["claimRef"]["namespace"], "aidash-pr-2")

    def test_destroy_deletes_retained_disks_but_only_releases_the_preview_disk(self):
        self.request("pr-1")
        self.reconcile()
        owned = {disk for disk in self.kube.disks if disk.startswith("pvc-aidash-pr-1-")}
        self.assertEqual(len(owned), 4)
        self.closed.add("pr-1")
        self.calls.clear()
        apply = self.cloud.apply

        def recorded_apply(managed, **kwargs):
            self.calls.append(("apply", bool(kwargs.get("retiring"))))
            apply(managed, **kwargs)

        with patch.object(self.cloud, "apply", side_effect=recorded_apply):
            self.reconcile()
        self.assertEqual(self.store.state["environments"]["pr-1"]["status"], "destroyed")
        self.assertEqual({call[1] for call in self.calls if call[0] == "delete_disk"}, owned)
        self.assertEqual(self.kube.disks, {"aidash-preview-tls"})
        self.assertIsNotNone(self.kube.get("persistentvolume", "aidash-preview-tls"))
        self.assertNotIn("claimRef", self.kube.get("persistentvolume", "aidash-preview-tls")["spec"])
        self.assertEqual([volume for volume in self.kube.items("persistentvolumes") if volume["metadata"]["name"] != "aidash-preview-tls"], [])
        for namespace in ("aidash-pr-1", "aidash-pr-1-sandbox", "aidash-pr-1-trusted"):
            self.assertLess(self.calls.index(("delete_namespace", namespace)), self.calls.index(("delete_disk", sorted(owned)[0])))
        self.assertLess(self.calls.index(("uninstall", "pr-1", "app")), self.calls.index(("delete_namespace", "aidash-pr-1")))
        self.assertLess(max(index for index, call in enumerate(self.calls) if call[0] == "delete_disk"), self.calls.index(("apply", True)))

    def test_destroy_refuses_an_unmanaged_retained_volume_before_deleting_anything(self):
        self.request("pr-1")
        self.reconcile()
        self.kube.put({"kind": "PersistentVolume", "metadata": {"name": "foreign"}, "spec": {
            "claimRef": {"namespace": "aidash-pr-1", "name": "data"}, "persistentVolumeReclaimPolicy": "Retain",
            "csi": {"volumeHandle": "projects/other-project/zones/us-central1-a/disks/data"},
        }})
        disks = set(self.kube.disks)
        self.closed.add("pr-1")
        self.calls.clear()
        with self.assertRaises(RuntimeError):
            self.reconcile()
        self.assertNotEqual(self.store.state["environments"]["pr-1"]["status"], "destroyed")
        self.assertFalse(any(call[0] in {"uninstall", "delete_namespace", "delete_disk"} for call in self.calls))
        self.assertEqual(self.kube.disks, disks)

    def test_pending_build_restores_broker_presealed_healthy_release(self):
        self.config["develop_branch"] = "develop/0.1.0"
        self.config["credential_brokers"] = {"develop": {"enabled": True, "image": "initial"}}
        self.request("develop", source_ref="develop/0.1.0")
        self.reconcile()
        self.request("develop", action="update", sha="b" * 40, source_ref="develop/0.1.0")["release"] = None
        self.assertEqual(self.store.state["environments"]["develop"]["sha"], "b" * 40)
        self.config["credential_brokers"]["develop"]["image"] = "rotated"
        self.calls.clear()
        self.reconcile()
        self.assertEqual(self.store.state["environments"]["develop"]["status"], "awaiting_build")
        self.assertLess(self.calls.index(("observe", "develop")), self.calls.index(("admission", "develop", "open")))
        self.assertTrue(self.cloud.managed["develop"]["running"])
        self.assertTrue(self.cloud.managed["develop"]["published"])
        self.assertEqual(self.cloud.managed["develop"]["release_sha"], SHA)
        self.assertFalse(self.deployed("develop") or self.stopped("develop"))

    def test_pending_build_reloads_changed_broker_settings_for_the_existing_release(self):
        self.config["develop_branch"] = "develop/0.1.0"
        initial = {"develop": {"enabled": True, "image": "initial", "signing_version": "1"}}
        self.config["credential_brokers"] = deepcopy(initial)

        def provider(identity):
            broker = self.cloud.brokers.get(identity, {})
            return descriptor(identity, broker.get("enabled"), broker.get("signing_version", "1"))

        with self.managed_outputs(provider):
            self.request("develop", source_ref="develop/0.1.0")
            self.reconcile()
            self.request("develop", action="update", sha="b" * 40, source_ref="develop/0.1.0")["release"] = None
            for desired in [{"develop": {"enabled": False, "image": "initial"}}, {}, {"develop": {"enabled": True, "image": "initial", "signing_version": "2"}}]:
                with self.subTest(desired=desired):
                    self.config["credential_brokers"] = deepcopy(initial)
                    self.reconcile()
                    self.config["credential_brokers"] = desired
                    self.calls.clear()
                    self.reconcile()
                    self.assertEqual([call for call in self.calls if call[0] in {"observe", "helm", "admission", "migrate"}], [
                        ("admission", "develop", "close"), ("observe", "develop"), ("helm", "develop", "app"), ("admission", "develop", "open"),
                    ])
                    entry = self.store.state["environments"]["develop"]
                    self.assertEqual(entry["status"], "awaiting_build")
                    self.assertEqual(entry["provider_credentials"], self.cloud.outputs()["develop"]["provider_credentials"])
                    self.assertEqual(self.kube.releases[("aidash-develop", "app")]["providerCredentials"]["settings"], entry["provider_credentials"])
                    self.assertEqual(self.kube.releases[("aidash-develop", "app")]["release"]["sourceSha"], SHA)
                    self.assertEqual(self.cloud.managed["develop"]["release_sha"], SHA)
                    self.assertTrue(self.cloud.managed["develop"]["published"])
                    self.assertFalse(self.stopped("develop"))
                    self.calls.clear()
                    self.cloud.plans.clear()
                    self.reconcile()
                    self.assertFalse(self.deployed("develop") or self.stopped("develop"))
                    self.assertEqual(self.cloud.plans, [])

    def test_pending_build_recovers_a_broker_apply_completed_by_an_interrupted_controller(self):
        self.config["develop_branch"] = "develop/0.1.0"
        self.config["credential_brokers"] = {"develop": {"enabled": True, "image": "initial"}}

        def provider(identity):
            return descriptor(identity, self.cloud.brokers.get(identity, {}).get("enabled"))

        with self.managed_outputs(provider):
            self.request("develop", source_ref="develop/0.1.0")
            self.reconcile()
            self.request("develop", action="update", sha="b" * 40, source_ref="develop/0.1.0")["release"] = None
            self.config["credential_brokers"] = {}
            # The prior controller applied broker intent, then exited before
            # reloading the old release. Terraform already matches desired intent.
            self.cloud.apply(self.cloud.managed)
            self.calls.clear()
            self.cloud.plans.clear()
            self.reconcile()
            self.assertEqual([call for call in self.calls if call[0] in {"observe", "helm", "admission"}], [
                ("admission", "develop", "close"), ("observe", "develop"), ("helm", "develop", "app"), ("admission", "develop", "open"),
            ])
            entry = self.store.state["environments"]["develop"]
            self.assertEqual(entry["status"], "awaiting_build")
            self.assertIsNone(entry["provider_credentials"]["broker"])
            self.assertEqual(self.cloud.managed["develop"]["release_sha"], SHA)
            self.assertEqual(self.cloud.plans, [])
            self.assertFalse(self.stopped("develop"))

    def test_pending_build_broker_settings_reload_failure_keeps_admission_gated(self):
        self.config["develop_branch"] = "develop/0.1.0"
        self.config["credential_brokers"] = {"develop": {"enabled": True, "image": "initial"}}
        self.request("develop", source_ref="develop/0.1.0")
        self.reconcile()
        self.request("develop", action="update", sha="b" * 40, source_ref="develop/0.1.0")["release"] = None
        self.config["credential_brokers"] = {}
        self.calls.clear()
        with self.managed_outputs(lambda identity: descriptor(identity, False)), self.fail_reload("develop"), self.assertRaisesRegex(RuntimeError, "Reconciliation incomplete"):
            self.reconcile()
        self.assertEqual(self.calls[-1:], [("admission", "develop", "close")])
        self.assertNotIn(("admission", "develop", "open"), self.calls)
        self.assertFalse(self.cloud.managed["develop"]["published"])
        entry = self.store.state["environments"]["develop"]
        self.assertEqual(entry["failed_deployment_generation"], entry["generation"])
        self.assertFalse(self.stopped("develop"))

    def test_declined_idle_stop_restores_broker_presealed_healthy_release(self):
        self.config["credential_brokers"] = {"test": {"enabled": True, "image": "initial"}}
        self.request()
        self.reconcile()
        self.config["credential_brokers"]["test"]["image"] = "rotated"
        self.idle = True
        self.kube.script = ["flags", "busy"]
        self.calls.clear()
        self.reconcile()
        self.assertEqual(self.calls, seal("test") + [("activity", "test")] + seal("test") + unseal("test") + unseal("test"))
        self.assertTrue(self.cloud.managed["test"]["running"])
        self.assertTrue(self.cloud.managed["test"]["published"])

    def test_broker_change_does_not_unseal_a_deliberately_failed_release(self):
        self.config["credential_brokers"] = {"test": {"enabled": True, "image": "initial"}}
        self.request()
        self.reconcile()
        self.request(action="resume", force=True)
        with patch.object(controller.kube, "deploy", side_effect=RuntimeError("chart unavailable")), self.assertRaises(RuntimeError):
            self.reconcile()
        self.assertFalse(self.cloud.managed["test"]["published"])
        self.config["credential_brokers"]["test"]["image"] = "rotated"
        self.calls.clear()
        self.reconcile()
        self.assertNotIn(("admission", "test", "open"), self.calls)
        self.assertFalse(self.cloud.managed["test"]["published"])
        self.assertFalse(self.deployed("test") or self.stopped("test"))

    def test_pending_build_keeps_observing_old_release_and_stops_when_idle(self):
        self.request("pr-1")
        self.reconcile()
        entry = self.request("pr-1", "update", sha="b" * 40)
        entry["release"] = None
        self.calls.clear()
        self.busy = True
        self.idle = True
        self.reconcile()
        self.assertTrue(self.cloud.managed["pr-1"]["running"])
        self.assertEqual(
            self.store.state["environments"]["pr-1"]["status"], "awaiting_build"
        )
        self.assertEqual(self.calls, [("activity", "pr-1")])
        self.busy = False
        self.reconcile()
        self.assertFalse(self.cloud.managed["pr-1"]["running"])
        self.assertEqual(self.cloud.managed["pr-1"]["release_sha"], SHA)
        self.assertFalse(self.cloud.managed["pr-1"]["published"])
        self.assertEqual(self.store.state["environments"]["pr-1"]["desired"], "stopped")
        self.assertFalse(self.deployed("pr-1"))
        self.calls.clear()
        self.reconcile()
        self.assertFalse(self.cloud.managed["pr-1"]["running"])
        self.assertEqual(self.calls, [])

    def test_stale_idle_snapshot_never_starts_a_drain(self):
        self.request()
        self.reconcile()
        self.idle = True
        self.calls.clear()
        with patch.object(self.kube, "snapshot", side_effect=lambda namespace, mode="flags": dict(
            FakeCluster.snapshot(self.kube, namespace, mode), observed_at=9000
        )):
            self.reconcile()
        self.assertEqual(self.calls, [("activity", "test")])
        self.assertTrue(self.cloud.managed["test"]["running"])

    def test_failed_deployment_waits_for_explicit_retry_and_keeps_data(self):
        self.request()
        with patch.object(controller.kube, "migrate", side_effect=RuntimeError("fixture migration failure")):
            with self.assertRaises(RuntimeError):
                self.reconcile()
        self.assertFalse(self.cloud.managed["test"]["published"])
        self.assertEqual(self.kube.replicas("aidash-test", "deployment", "app-aidash-server"), 0)
        self.calls.clear()
        self.reconcile()
        self.assertFalse(self.deployed("test"))
        self.request(action="resume", force=True)
        self.reconcile()
        self.assertEqual(self.store.state["environments"]["test"]["status"], "ready")
        self.assertIn(("helm", "test", "app"), self.calls)
        self.assertFalse(any(retiring for _, retiring in self.cloud.plans))

    def test_stop_waits_for_work_and_destroy_only_retires_its_owned_environment(self):
        self.request()
        self.reconcile()
        self.request("pr-1")
        self.reconcile()
        self.request(action="stop")
        self.busy = True
        self.reconcile()
        self.assertTrue(self.cloud.managed["test"]["running"])
        self.assertEqual(self.store.state["environments"]["test"]["status"], "draining")
        self.request(action="destroy")
        self.reconcile()
        self.assertNotIn("test", self.cloud.managed)
        self.assertIn("pr-1", self.cloud.managed)
        self.assertNotIn(("delete_namespace", "aidash-pr-1"), self.calls)
        self.assertEqual(
            [retiring for _, retiring in self.cloud.plans if retiring], [{"test"}]
        )

    def test_deadline_fences_an_incomplete_deployment_and_releases_the_lock(self):
        self.request()
        with patch.object(
            controller.kube,
            "deploy",
            side_effect=controller.OperationDeadline("fixture deadline"),
        ):
            with self.assertRaises(controller.OperationDeadline):
                self.reconcile()
        self.assertFalse(self.store.locked)
        entry = self.store.state["environments"]["test"]
        self.assertEqual(entry["failed_deployment_generation"], entry["generation"])
        self.assertFalse(self.cloud.managed["test"]["published"])
        self.calls.clear()
        self.reconcile()
        self.assertFalse(self.deployed("test"))

    def test_closed_pr_cleans_up_even_when_another_environment_is_unhealthy(self):
        self.request()
        self.reconcile()
        self.request("pr-1")
        self.reconcile()
        self.closed.add("pr-1")
        with patch.object(
            controller.kube, "activity", side_effect=RuntimeError("fixture unavailable")
        ):
            with self.assertRaises(RuntimeError):
                self.reconcile()
        self.assertNotIn("pr-1", self.cloud.managed)
        self.assertEqual(
            self.store.state["environments"]["pr-1"]["status"], "destroyed"
        )

    def test_new_request_wins_over_idle_decision(self):
        self.request()
        self.reconcile()
        self.idle = True
        self.kube.on_observe = lambda namespace: self.request(action="resume")
        with self.assertRaises(RuntimeError):
            self.reconcile()
        self.assertFalse(self.stopped("test"))
        self.assertTrue(self.cloud.managed["test"]["running"])
        self.assertEqual(self.store.state["environments"]["test"]["desired"], "running")
        self.kube.on_observe = None
        self.idle = False
        self.calls.clear()
        self.reconcile()
        self.assertEqual(self.calls[-3:], unseal("test"))
        self.assertEqual(self.store.state["environments"]["test"]["status"], "ready")

    def test_stop_cannot_be_accepted_during_a_create_apply(self):
        self.request()
        original = self.cloud.apply
        attempts = []
        with TemporaryDirectory() as directory:
            event = Path(directory) / "event.json"
            event.write_text(
                json.dumps({"inputs": {"environment": "test", "action": "stop"}})
            )
            with (
                patch.dict(
                    os.environ,
                    {
                        "GITHUB_EVENT_PATH": str(event),
                        "GITHUB_EVENT_NAME": "workflow_dispatch",
                        "GITHUB_ACTOR": "fixture",
                        "GITHUB_RUN_ID": "2",
                        "RUNNER_TEMP": directory,
                    },
                ),
                patch.object(controller, "permission", return_value=None),
            ):

                def racing_apply(*args, **kwargs):
                    before = deepcopy(self.store.state)
                    with self.assertRaisesRegex(RuntimeError, "lifecycle lock is busy"):
                        controller.prepare(CONFIG, self.store)
                    self.assertEqual(self.store.state, before)
                    attempts.append(True)
                    original(*args, **kwargs)

                with patch.object(self.cloud, "apply", racing_apply):
                    self.reconcile()
                self.assertTrue(attempts)
                controller.prepare(CONFIG, self.store)
                self.assertEqual(
                    self.store.state["environments"]["test"]["desired"], "stopped"
                )
                self.cloud.plans.clear()
                self.calls.clear()
                self.reconcile()
                self.assertFalse(self.cloud.managed["test"]["running"])
                self.assertFalse(any(call[0] == "helm" for call in self.calls))

    def test_retry_rebuilds_pending_intent_without_overwriting_a_newer_request(self):
        with TemporaryDirectory() as directory:
            event = Path(directory) / "event.json"
            event.write_text(
                json.dumps(
                    {
                        "inputs": {
                            "environment": "test",
                            "action": "create",
                            "source_ref": "main",
                        }
                    }
                )
            )
            with (
                patch.dict(
                    os.environ,
                    {
                        "GITHUB_EVENT_PATH": str(event),
                        "GITHUB_EVENT_NAME": "workflow_dispatch",
                        "GITHUB_ACTOR": "fixture",
                        "GITHUB_RUN_ID": "2",
                        "GITHUB_OUTPUT": str(Path(directory) / "outputs"),
                        "RUNNER_TEMP": directory,
                    },
                ),
                patch.object(controller, "permission"),
                patch.object(
                    controller,
                    "source",
                    return_value={
                        "sha": SHA,
                        "source_ref": "main",
                        "source_repo": CONFIG["repository"],
                    },
                ) as source,
            ):
                destination = Path(directory) / "aidash-request.json"
                controller.prepare(CONFIG, self.store)
                accepted = deepcopy(self.store.state)
                original = json.loads(destination.read_text())
                self.assertTrue(original["build"])
                # The source branch can move between workflow attempts; retry
                # the accepted SHA/generation, without accepting a new intent.
                source.return_value = dict(source.return_value, sha="b" * 40)
                controller.prepare(CONFIG, self.store)
                self.assertEqual(json.loads(destination.read_text()), original)
                self.assertEqual(self.store.state, accepted)
                self.store.state = transition(
                    self.store.state,
                    {"environment": "test", "action": "stop", "sequence": 3},
                    "unused",
                    1,
                )[0]
                stopped = deepcopy(self.store.state)
                controller.prepare(CONFIG, self.store)
                self.assertFalse(json.loads(destination.read_text())["build"])
                self.assertEqual(self.store.state, stopped)

    def test_accepted_stop_fences_an_older_create_before_any_apply(self):
        self.request()
        self.request(action="stop")
        self.reconcile()
        self.assertFalse(self.cloud.plans)
        self.assertFalse(self.calls)


class IntakeTests(unittest.TestCase):
    def test_comment_preflight_rejects_noise_and_unauthorized_commands_without_cloud_access(
        self,
    ):
        with TemporaryDirectory() as directory:
            event = Path(directory) / "event.json"
            output = Path(directory) / "output"
            with (
                patch.dict(
                    os.environ,
                    {
                        "GITHUB_EVENT_PATH": str(event),
                        "GITHUB_EVENT_NAME": "issue_comment",
                        "GITHUB_REPOSITORY": CONFIG["repository"],
                        "GITHUB_OUTPUT": str(output),
                    },
                ),
                patch.object(sys, "argv", ["controller.py", "preflight"]),
                patch.object(
                    controller,
                    "load_config",
                    side_effect=AssertionError(
                        "preflight must not load cloud configuration"
                    ),
                ),
                patch.object(
                    controller,
                    "Store",
                    side_effect=AssertionError("preflight must not access cloud state"),
                ),
                patch.object(
                    controller, "github", return_value={"permission": "read"}
                ) as github,
            ):
                for body, pr, permission, allowed in (
                    ("Thanks!", True, "write", False),
                    ("/preview up\necho bad", True, "write", False),
                    ("/preview stop spot", True, "write", False),
                    ("/preview up", False, "write", False),
                    ("/preview up", True, "read", False),
                    ("/preview up normal", True, "write", True),
                    ("/preview stop", True, "maintain", True),
                ):
                    with self.subTest(body=body, pr=pr, permission=permission):
                        event.write_text(
                            json.dumps(
                                {
                                    "action": "created",
                                    "issue": {"pull_request": {}} if pr else {},
                                    "comment": {
                                        "body": body,
                                        "user": {"login": "fixture"},
                                    },
                                }
                            )
                        )
                        output.write_text("")
                        github.return_value = {"permission": permission}
                        controller.main()
                        self.assertEqual(
                            output.read_text(), f"proceed={str(allowed).lower()}\n"
                        )

    def test_job_setup_time_reduces_the_operation_budget(self):
        with (
            patch.dict(
                os.environ,
                {
                    "GITHUB_ACTIONS": "true",
                    "GITHUB_REPOSITORY": CONFIG["repository"],
                    "GITHUB_RUN_ID": "1",
                    "GITHUB_RUN_ATTEMPT": "1",
                    "GITHUB_JOB": "apply",
                },
            ),
            patch.object(
                controller,
                "github",
                return_value={
                    "jobs": [{"name": "apply", "started_at": "2026-09-28T00:00:00Z"}]
                },
            ),
            patch.object(controller.time, "time", return_value=1790553600 + 45 * 60),
        ):
            self.assertEqual(controller.controller_budget("reconcile"), 3 * 60)
            with patch.object(
                controller.time, "time", return_value=1790553600 + 50 * 60
            ):
                with self.assertRaisesRegex(
                    controller.Refused, "Insufficient job time"
                ):
                    controller.controller_budget("reconcile")


if __name__ == "__main__":
    unittest.main()
