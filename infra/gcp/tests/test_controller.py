"""Exercise complete reconciliation with cloud boundaries replaced, not the policy."""

from contextlib import ExitStack, contextmanager, redirect_stdout
from copy import deepcopy
import io
import json
import os
from pathlib import Path
import sys
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "control"))
import controller
from cloud import Terraform as NativeTerraform
from policy import transition


SHA = "a" * 40
CONFIG = {"repository": "kent8192/aidash"}


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
        self.status = {}
        self.plans = []
        self.configuration = {}
        self.brokers = {}
        self.gcip = {}
        self.resource_changes = []
        self.output_changes = {}

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
        self.plans.append((deepcopy(managed), set(retiring)))
        for identity, value in managed.items():
            if value.get("vm_present", True) and (
                identity not in self.managed
                or not self.managed[identity].get("vm_present", True)
            ):
                self.status[identity] = "RUNNING"
        self.managed = deepcopy(managed)
        self.brokers = deepcopy(self.broker_configuration(managed))

    def outputs(self):
        return {
            key: dict(
                instance=key,
                zone="us-central1-a",
                hostname="test.aidash.run",
                runtime_secret=key,
                gcip=deepcopy(self.gcip),
            )
            for key in self.managed
        }


class ReconcileTests(unittest.TestCase):
    def setUp(self):
        self.store = MemoryStore()
        self.cloud = CloudFixture()
        self.config = deepcopy(CONFIG)
        self.sequence = 0
        self.calls = []
        self.busy = False
        self.idle = False
        self.closed = set()
        self.retirement_failure = False
        self.context = ExitStack()
        self.addCleanup(self.context.close)
        replacements = {
            "Terraform": self.terraform,
            "instance_status": lambda config, output: self.cloud.status.get(
                output["instance"], "MISSING"
            ),
            "host": self.host,
            "power": self.power,
            "bundle": lambda *args: ("bundles/" + "b" * 64 + ".tar.gz", "b" * 64),
            "ci_success": lambda *args: None,
            "provision_secret": lambda *args: None,
            "restart_bootstrap": lambda *args: self.calls.append(
                ("bootstrap", args[1]["instance"], args[2])
            ),
            "public_health": lambda *args: None,
            "github": self.github,
            "retire_provider_credentials": self.retire_credentials,
        }
        for key, value in replacements.items():
            self.context.enter_context(patch.object(controller, key, value))
        self.context.enter_context(
            patch.object(controller.time, "time", return_value=10000)
        )

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

    def host(self, config, output, action, force=False):
        self.calls.append((action, output["instance"]))
        if action.startswith("seal"):
            return {"sealed": force or not self.busy}
        if action == "quiesce":
            return {"quiesced": True}
        if action == "health":
            return {"source_sha": self.cloud.managed[output["instance"]]["release_sha"]}
        return dict(
            protocol="aidash-infra-activity/1",
            busy=self.busy,
            observed_at=10000,
            last_active=0 if self.idle else 10000,
        )

    def power(self, config, output, action):
        self.calls.append((action, output["instance"]))
        self.cloud.status[output["instance"]] = (
            "RUNNING" if action == "start" else "TERMINATED"
        )

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
            entry["release"] = {"source_sha": entry["sha"], "images": {}}
        return entry

    def retire_credentials(self, config, identity):
        if config.get("byok_project_id"):
            self.calls.append(("retire_secrets", identity))
            if self.retirement_failure:
                raise RuntimeError("retirement inventory unavailable")

    def reconcile(self, byok=False):
        config = dict(self.config, byok_project_id="fixture-byok") if byok else self.config
        controller.reconcile(config, self.store)

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
                self.assertFalse(
                    any(
                        call[0] in {"bootstrap", "start", "stop"}
                        for call in self.calls
                    )
                )
                drain_required = previously_enabled or bool((desired or {}).get("enabled"))
                self.assertEqual(("seal", "test") in self.calls, drain_required)
                self.assertEqual(("unseal", "test") in self.calls, drain_required)
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
                self.assertEqual(self.calls, [("seal", "test")])
                self.busy = False
                self.calls.clear()
                apply = self.cloud.apply
                def recorded_apply(managed, apply=apply, **kwargs):
                    self.calls.append(("broker_apply", "test"))
                    apply(managed, **kwargs)
                with patch.object(self.cloud, "apply", side_effect=recorded_apply):
                    self.reconcile()
                self.assertLess(self.calls.index(("seal", "test")), self.calls.index(("broker_apply", "test")))
                self.assertIn(("unseal", "test"), self.calls)
                self.assertFalse(any(call[0] in {"start", "stop", "bootstrap"} for call in self.calls))
                self.assertEqual(self.cloud.brokers, desired)
                self.cloud.plans.clear()
                self.calls.clear()
                self.reconcile()
                self.assertEqual(self.cloud.plans, [])

    def test_interrupted_vm_cannot_apply_a_busy_environments_broker_removal(self):
        broker = {"enabled": True, "image": "initial"}
        self.config["credential_brokers"] = {"test": broker}
        self.request()
        self.reconcile()
        self.request("pr-1")
        self.reconcile()
        self.cloud.status["pr-1"] = "MISSING"
        self.config["credential_brokers"] = {}
        self.cloud.plans.clear()
        self.calls.clear()
        self.busy = True
        self.reconcile()
        self.assertEqual(self.cloud.plans, [])
        self.assertEqual(self.cloud.brokers, {"test": broker})
        self.busy = False
        self.calls.clear()
        self.reconcile()
        self.assertEqual(self.cloud.brokers, {})
        self.assertTrue(self.cloud.plans)
        self.assertTrue(all(not plan[0]["pr-1"]["vm_present"] for plan in self.cloud.plans))
        self.assertFalse(any(call[0] in {"start", "stop", "bootstrap"} for call in self.calls))

    def test_project_change_refuses_before_observation_cleanup_or_apply(self):
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
            patch.object(controller, "observe_interruptions") as observe,
        ):
            for configured in ["", "replacement-byok"]:
                with self.subTest(configured=configured):
                    self.cloud.configuration["byok_project_id"] = configured
                    with self.assertRaisesRegex(RuntimeError, "Restore the applied BYOK project"):
                        controller.reconcile(self.cloud.configuration, self.store)
            observe.assert_not_called()
        self.assertEqual(self.calls, [])
        self.assertEqual(self.cloud.plans, plans)
        self.assertEqual(self.store.state, before)

    def test_blocked_multi_environment_broker_change_restores_sealed_hosts(self):
        brokers = {identity: {"enabled": True, "image": "initial"} for identity in ["develop", "test"]}
        self.config["develop_branch"] = "develop/0.1.0"
        self.config["credential_brokers"] = brokers
        for identity in brokers:
            self.request(identity, source_ref="develop/0.1.0" if identity == "develop" else "main")
            self.reconcile()
        for busy in brokers:
            with self.subTest(busy=busy):
                self.config["credential_brokers"] = brokers
                self.reconcile()
                self.config["credential_brokers"] = {}
                self.cloud.plans.clear()
                self.calls.clear()
                def selective_host(config, output, action, force=False, busy=busy):
                    result = self.host(config, output, action, force)
                    return {"sealed": False} if action == "seal" and output["instance"] == busy else result
                with patch.object(controller, "host", side_effect=selective_host):
                    self.reconcile()
                self.assertEqual(self.cloud.plans, [])
                self.assertEqual(self.cloud.brokers, brokers)
                idle = next(identity for identity in brokers if identity != busy)
                self.assertEqual(self.calls, [("seal", "develop"), ("seal", "test"), ("unseal", idle)])
                self.assertEqual(self.store.state["environments"][busy]["status"], "waiting_for_active_work")

    def test_broker_preflight_failure_restores_prior_seals(self):
        brokers = {identity: {"enabled": True, "image": "initial"} for identity in ["develop", "test"]}
        self.config["develop_branch"] = "develop/0.1.0"
        self.config["credential_brokers"] = brokers
        for identity in brokers:
            self.request(identity, source_ref="develop/0.1.0" if identity == "develop" else "main")
            self.reconcile()
        self.config["credential_brokers"] = {}
        self.cloud.plans.clear()
        self.calls.clear()
        def failed_seal(config, output, action, force=False):
            result = self.host(config, output, action, force)
            if action == "seal" and output["instance"] == "test":
                raise RuntimeError("seal unavailable")
            return result
        with patch.object(controller, "host", side_effect=failed_seal), self.assertRaisesRegex(RuntimeError, "seal unavailable"):
            self.reconcile()
        self.assertEqual(self.calls, [("seal", "develop"), ("seal", "test"), ("unseal", "develop")])
        self.assertEqual(self.cloud.plans, [])
        self.assertEqual(self.cloud.brokers, brokers)

    def test_broker_apply_failure_restores_all_presealed_hosts_and_can_retry(self):
        brokers = {identity: {"enabled": True, "image": "initial"} for identity in ["develop", "test"]}
        self.config["develop_branch"] = "develop/0.1.0"
        self.config["credential_brokers"] = brokers
        for identity in brokers:
            self.request(identity, source_ref="develop/0.1.0" if identity == "develop" else "main")
            self.reconcile()
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
                self.assertEqual(self.calls, [("seal", "develop"), ("seal", "test"), ("apply", "all"), ("unseal", "develop"), ("unseal", "test")])
                self.assertEqual(self.cloud.managed, managed)
                self.assertEqual(self.cloud.brokers, brokers)
                self.calls.clear()
                self.reconcile()
                self.assertEqual(self.cloud.brokers, desired)
                self.assertFalse(any(call[0] in {"start", "stop", "bootstrap"} for call in self.calls))

    def test_failed_broker_apply_restoration_still_attempts_every_host(self):
        brokers = {identity: {"enabled": True, "image": "initial"} for identity in ["develop", "test"]}
        self.config["develop_branch"] = "develop/0.1.0"
        self.config["credential_brokers"] = brokers
        for identity in brokers:
            self.request(identity, source_ref="develop/0.1.0" if identity == "develop" else "main")
            self.reconcile()
        self.config["credential_brokers"] = {}
        self.calls.clear()
        def failed_restore(config, output, action, force=False):
            result = self.host(config, output, action, force)
            if action == "unseal" and output["instance"] == "develop":
                raise RuntimeError("unseal unavailable")
            return result
        with patch.object(self.cloud, "apply", side_effect=RuntimeError("apply unavailable")), patch.object(controller, "host", side_effect=failed_restore), self.assertRaisesRegex(RuntimeError, "admission restoration failed") as failure:
            self.reconcile()
        self.assertEqual(self.calls, [("seal", "develop"), ("seal", "test"), ("unseal", "develop"), ("unseal", "test")])
        self.assertIn("apply unavailable", str(failure.exception.__context__))

    def test_broker_deadline_restores_presealed_hosts_with_a_fresh_cleanup_budget(self):
        from cloud import DEADLINE
        brokers = {identity: {"enabled": True, "image": "initial"} for identity in ["develop", "test"]}
        self.config["develop_branch"] = "develop/0.1.0"
        self.config["credential_brokers"] = brokers
        for identity in brokers:
            self.request(identity, source_ref="develop/0.1.0" if identity == "develop" else "main")
            self.reconcile()
        for stage in ["preflight", "apply"]:
            with self.subTest(stage=stage):
                self.config["credential_brokers"] = {}
                self.calls.clear()
                def deadline_host(config, output, action, force=False, stage=stage):
                    result = self.host(config, output, action, force)
                    if stage == "preflight" and action == "seal" and output["instance"] == "test":
                        DEADLINE.set(0)
                        controller.bounded_timeout(1)
                    if action == "unseal":
                        self.assertGreater(controller.bounded_timeout(1), 0)
                    return result
                def deadline_apply(*args, **kwargs):
                    self.calls.append(("apply", "all"))
                    DEADLINE.set(0)
                    controller.bounded_timeout(1)
                with controller.operation_budget(60), patch.object(controller, "host", side_effect=deadline_host), patch.object(self.cloud, "apply", side_effect=deadline_apply), self.assertRaises(controller.OperationDeadline):
                    self.reconcile()
                expected = [("seal", "develop"), ("seal", "test")]
                if stage == "apply":
                    expected += [("apply", "all")]
                expected += [("unseal", "develop")]
                if stage == "apply":
                    expected += [("unseal", "test")]
                self.assertEqual(self.calls, expected)
                self.assertEqual(self.cloud.brokers, brokers)
                self.assertFalse(self.store.locked)

    def test_broker_restore_deadline_still_attempts_every_host(self):
        outputs = {identity: {"instance": identity} for identity in ["develop", "test"]}
        def deadline_restore(config, output, action, force=False):
            self.calls.append((action, output["instance"]))
            if output["instance"] == "develop":
                raise controller.OperationDeadline("fixture cleanup deadline")
        with patch.object(controller, "host", side_effect=deadline_restore), self.assertRaisesRegex(RuntimeError, "admission restoration failed"):
            controller.restore_broker_admission(self.config, outputs, set(outputs))
        self.assertEqual(self.calls, [("unseal", "develop"), ("unseal", "test")])

    def test_interruption_apply_failure_without_broker_seals_preserves_original_error(self):
        self.request()
        self.reconcile()
        self.cloud.status["test"] = "TERMINATED"
        self.calls.clear()
        with patch.object(self.cloud, "apply", side_effect=RuntimeError("apply unavailable")), self.assertRaisesRegex(RuntimeError, "apply unavailable"):
            self.reconcile()
        self.assertEqual(self.calls, [])

    def test_broker_admission_restoration_attempts_all_hosts_after_a_failure(self):
        calls = []
        def restore(config, output, action):
            calls.append((action, output["instance"]))
            if output["instance"] == "develop":
                raise RuntimeError("unseal unavailable")
        outputs = {identity: {"instance": identity} for identity in ["develop", "test"]}
        with patch.object(controller, "host", side_effect=restore), self.assertRaisesRegex(RuntimeError, "admission restoration failed"):
            controller.restore_broker_admission(self.config, outputs, set(outputs))
        self.assertEqual(calls, [("unseal", "develop"), ("unseal", "test")])

    def test_byok_retirement_cleans_stopped_and_missing_vms_without_waking_them(self):
        for status in ["TERMINATED", "MISSING"]:
            with self.subTest(status=status):
                self.request()
                self.reconcile()
                self.cloud.managed["test"]["running"] = False
                self.cloud.status["test"] = status
                self.request(action="destroy")
                self.calls.clear()
                apply = self.cloud.apply

                def recorded_apply(managed, apply=apply, status=status, **kwargs):
                    self.calls.append(("apply", bool(kwargs.get("retiring"))))
                    if kwargs.get("retiring") or "test" not in managed:
                        self.assertIn(("retire_secrets", "test"), self.calls)
                    if "test" in managed and status == "MISSING":
                        self.assertFalse(managed["test"]["vm_present"])
                    apply(managed, **kwargs)

                with patch.object(self.cloud, "apply", side_effect=recorded_apply):
                    self.reconcile(byok=True)
                self.assertNotIn("test", self.cloud.managed)
                self.assertNotIn(("start", "test"), self.calls)
                self.assertNotIn(("seal", "test"), self.calls)
                self.assertFalse(any(call[0] == "bootstrap" for call in self.calls))
                self.assertLess(self.calls.index(("retire_secrets", "test")), self.calls.index(("apply", True)))

    def test_byok_cleanup_failure_preserves_identity_and_disk_then_retries(self):
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
        self.assertLess(self.calls.index(("seal", "test")), self.calls.index(("retire_secrets", "test")))
        self.assertNotIn(("unseal", "test"), self.calls)
        self.retirement_failure = False
        self.reconcile(byok=True)
        self.assertNotIn("test", self.cloud.managed)

    def test_partial_byok_retirement_still_cleans_without_a_managed_vm(self):
        self.request()
        self.reconcile()
        self.request(action="destroy")
        del self.cloud.managed["test"]
        self.calls.clear()
        self.reconcile(byok=True)
        self.assertEqual(self.calls, [("retire_secrets", "test")])
        self.assertEqual(self.store.state["environments"]["test"]["status"], "destroyed")

    def test_resume_of_running_host_does_not_authorize_a_future_spot_restart(self):
        self.request()
        self.reconcile()
        self.request(action="resume")
        self.reconcile()
        self.assertFalse(self.store.state["environments"]["test"]["start_pending"])
        self.cloud.status["test"] = "TERMINATED"
        self.reconcile()
        self.assertEqual(self.store.state["environments"]["test"]["desired"], "stopped")
        self.assertNotIn(("start", "test"), self.calls)
        self.assertFalse(self.cloud.managed["test"]["published"])

    def test_provider_descriptor_change_waits_for_work_then_redeploys_once_without_power(self):
        self.request()
        self.reconcile()
        outputs = self.cloud.outputs
        descriptor = {"store": {"environment_id": "test"}, "broker": {"endpoint": "https://broker.run.app/api/v1", "kid": "version-1"}}
        def managed_outputs():
            values = outputs()
            values["test"]["provider_credentials"] = descriptor if self.cloud.brokers.get("test", {}).get("enabled") else None
            return values
        with patch.object(self.cloud, "outputs", side_effect=managed_outputs):
            for enabled in [True, False]:
                self.config["credential_brokers"] = {"test": {"enabled": enabled, "image": "fixture"}}
                self.calls.clear()
                self.busy = True
                self.reconcile()
                self.assertEqual(self.store.state["environments"]["test"]["status"], "waiting_for_active_work")
                self.assertFalse(any(call[0] in {"bootstrap", "start", "stop"} for call in self.calls))
                self.busy = False
                self.calls.clear()
                self.reconcile()
                entry = self.store.state["environments"]["test"]
                self.assertEqual(entry["status"], "ready")
                self.assertEqual(entry["provider_credentials"], descriptor if enabled else None)
                self.assertEqual([call for call in self.calls if call[0] == "bootstrap"], [("bootstrap", "test", False)])
                self.assertFalse(any(call[0] in {"start", "stop"} for call in self.calls))
                self.calls.clear()
                self.cloud.plans.clear()
                self.reconcile()
                self.assertEqual(self.cloud.plans, [])
                self.assertFalse(any(call[0] == "bootstrap" for call in self.calls))

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
            # Every published host must be fenced before the shared input is
            # applied, including one processed later in the environment loop.
            for identity in ("pr-1", "develop"):
                self.assertIn(("quiesce", identity), self.calls)
            original_apply(*args, **kwargs)
            self.cloud.gcip = {
                "tenant_ids": ["pool"] if config["gcip_tenants"] else [],
                "tenant_bindings": {
                    "pool": config["gcip_tenants"]["company"]["tenant"]
                } if config["gcip_tenants"] else {},
            }

        def provision(config, output, kind):
            secrets[output["instance"]] = deepcopy(output["gcip"])

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
                        self.assertIn(("bootstrap", identity, False), self.calls)
                        self.assertIn(("unseal", identity), self.calls)
                    self.assertFalse(any(call[0] == "start" for call in self.calls))
                    self.assertNotIn(("bootstrap", "test", False), self.calls)
                    self.assertEqual(self.cloud.status["test"], "TERMINATED")
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
                    self.assertFalse(any(call[0] == "bootstrap" for call in self.calls))

    def test_failed_gcip_refresh_stays_gated_and_retries_without_restarting_successes(self):
        self.request()
        self.reconcile()
        self.request("pr-1")
        self.reconcile()
        config = dict(CONFIG, gcip_tenants={"company": {"tenant": "new"}})

        def restart(config, output, fresh_boot):
            self.calls.append(("bootstrap", output["instance"], fresh_boot))
            if output["instance"] == "test":
                raise RuntimeError("fixture reload failure")

        self.calls.clear()
        with patch.object(controller, "restart_bootstrap", restart):
            with self.assertRaisesRegex(RuntimeError, "Reconciliation incomplete"):
                controller.reconcile(config, self.store)
        self.assertNotIn(("unseal", "test"), self.calls)
        failed_host_calls = [call for call in self.calls if call[1] == "test"]
        self.assertEqual(failed_host_calls[-1], ("quiesce", "test"))
        self.assertEqual(failed_host_calls.count(("quiesce", "test")), 2)
        self.assertIn(("unseal", "pr-1"), self.calls)
        self.assertTrue(self.store.state["environments"]["test"]["gcip_pending"])
        self.assertFalse(self.store.state["environments"]["pr-1"]["gcip_pending"])
        self.calls.clear()
        controller.reconcile(config, self.store)
        self.assertIn(("bootstrap", "test", False), self.calls)
        self.assertNotIn(("bootstrap", "pr-1", False), self.calls)
        self.assertFalse(self.store.state["environments"]["test"]["gcip_pending"])

    def test_unconfirmed_gcip_quiescence_aborts_before_shared_apply(self):
        self.request()
        self.reconcile()
        config = dict(CONFIG, gcip_tenants={"company": {"tenant": "new"}})
        plans = len(self.cloud.plans)
        original = self.host

        def unconfirmed(config, output, action, force=False):
            if action == "quiesce":
                self.calls.append((action, output["instance"]))
                return {"quiesced": False}
            return original(config, output, action, force)

        with patch.object(controller, "host", unconfirmed):
            with self.assertRaisesRegex(controller.Refused, "quiescence was not confirmed"):
                controller.reconcile(config, self.store)
        self.assertEqual(len(self.cloud.plans), plans)
        self.assertTrue(self.store.state["environments"]["test"]["gcip_pending"])

    def test_tenant_mfa_is_enforced_every_run_and_fences_only_after_a_failed_correction(self):
        self.request()
        self.reconcile()
        config = dict(CONFIG, gcip_tenants={"company": {"tenant": "acme"}})
        self.cloud.gcip = {
            "project_id": "aidash-fixture",
            "tenant_ids": ["pool"],
            "tenant_bindings": {"pool": "acme"},
            "mfa": {"pool": "disabled"},
        }
        entry = lambda: self.store.state["environments"]["test"]
        drift = {}
        failing = []

        def mfa(output):
            self.calls.append(("mfa", output["instance"]))
            if failing:
                raise RuntimeError("fixture MFA failure")
            return dict(drift)

        with (
            patch.object(controller, "reconcile_mfa", mfa),
            patch.object(controller, "reconcile_environment"),
            patch.dict(os.environ, {"AIDASH_GCIP_IDP_SECRETS": "{}"}),
        ):
            self.calls.clear()
            controller.reconcile(config, self.store)
            # A new pool holds its declared MFA before the host reopens admission.
            self.assertLess(self.calls.index(("mfa", "test")), self.calls.index(("unseal", "test")))

            # Declaring the requirement explicitly changes neither fence digest.
            config["gcip_tenants"]["company"]["mfa"] = {"state": "disabled"}
            self.calls.clear()
            plans = len(self.cloud.plans)
            controller.reconcile(config, self.store)
            self.assertEqual([call for call in self.calls if call[0] != "observe"], [("mfa", "test")])
            self.assertEqual(len(self.cloud.plans), plans)
            self.assertNotIn("gcip_mfa_drift_at", entry())

            # Out-of-band drift is corrected, annotated and recorded without fencing.
            drift["pool"] = ["providerConfigs", "state"]
            self.calls.clear()
            with redirect_stdout(io.StringIO()) as stdout:
                controller.reconcile(config, self.store)
            self.assertIn(
                "::warning title=GCIP MFA drift corrected::test tenant=pool fields=providerConfigs,state",
                stdout.getvalue(),
            )
            self.assertEqual(entry()["gcip_mfa_drift_at"], 10000)
            self.assertEqual([call for call in self.calls if call[0] != "observe"], [("mfa", "test")])

            # A failed correction fences the host and retries through the GCIP refresh.
            drift.clear()
            failing.append(True)
            with self.assertRaisesRegex(RuntimeError, "Reconciliation incomplete"):
                controller.reconcile(config, self.store)
            self.assertTrue(entry()["gcip_pending"])
            failing.clear()
            self.calls.clear()
            controller.reconcile(config, self.store)
            self.assertLess(self.calls.index(("quiesce", "test")), self.calls.index(("mfa", "test")))
            self.assertLess(self.calls.index(("mfa", "test")), self.calls.index(("unseal", "test")))
            self.assertFalse(entry()["gcip_pending"])

    def test_gcip_change_fences_a_running_unpublished_failed_deployment(self):
        self.request()
        self.reconcile()
        self.cloud.managed["test"]["published"] = False
        entry = self.store.state["environments"]["test"]
        entry["failed_deployment_generation"] = entry["generation"]
        self.calls.clear()
        original = self.cloud.apply

        def apply(*args, **kwargs):
            self.assertIn(("quiesce", "test"), self.calls)
            original(*args, **kwargs)

        with patch.object(self.cloud, "apply", apply):
            controller.reconcile(dict(CONFIG, gcip_tenants={"company": {"tenant": "new"}}), self.store)
        self.assertNotIn(("unseal", "test"), self.calls)
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
            self.assertIn(("quiesce", "test"), self.calls)
            self.cloud.gcip = {"tenant_ids": ["new-pool"], "tenant_bindings": {"new-pool": "acme"}}
            self.cloud.resource_changes = []
            self.cloud.output_changes = {}

        def provision(config, output, kind):
            secrets[output["instance"]] = deepcopy(output["gcip"])

        with patch.object(self.cloud, "apply", apply), patch.object(controller, "provision_secret", provision), patch.object(controller, "reconcile_environment") as iam:
            controller.reconcile(config, self.store)
        self.assertEqual(secrets["test"]["tenant_ids"], ["new-pool"])
        self.assertEqual(secrets["develop"]["tenant_ids"], ["new-pool"])
        self.assertIn(("bootstrap", "test", False), self.calls)
        self.assertNotIn(("bootstrap", "develop", False), self.calls)
        self.assertNotIn(("start", "develop"), self.calls)
        self.assertTrue(iam.called)
        self.calls.clear()
        plans = len(self.cloud.plans)
        controller.reconcile(config, self.store)
        self.assertEqual(len(self.cloud.plans), plans)
        self.assertFalse(any(call[0] in {"quiesce", "bootstrap"} for call in self.calls))

    def test_gcip_refresh_failure_cannot_unseal_through_an_ordinary_stop(self):
        self.request()
        self.reconcile()
        self.request(action="stop")
        config = dict(CONFIG, gcip_tenants={"company": {"tenant": "new"}})
        with patch.object(controller, "restart_bootstrap", side_effect=RuntimeError("fixture reload failure")):
            self.calls.clear()
            with self.assertRaisesRegex(RuntimeError, "Reconciliation incomplete"):
                controller.reconcile(config, self.store)
            self.assertNotIn(("seal", "test"), self.calls)
            self.assertNotIn(("unseal", "test"), self.calls)
            self.assertNotIn(("stop", "test"), self.calls)
            self.assertTrue(self.store.state["environments"]["test"]["gcip_quiesced"])
            self.request(action="stop", force=True)
            self.calls.clear()
            with self.assertRaisesRegex(RuntimeError, "Reconciliation incomplete"):
                controller.reconcile(config, self.store)
            self.assertIn(("stop", "test"), self.calls)
            self.assertNotIn(("unseal", "test"), self.calls)
            self.assertEqual(self.cloud.status["test"], "TERMINATED")

    def test_failed_gcip_refresh_does_not_block_closed_pr_retirement(self):
        self.request()
        self.reconcile()
        self.request("pr-1")
        self.reconcile()
        self.closed.add("pr-1")
        self.calls.clear()
        config = dict(CONFIG, gcip_tenants={"company": {"tenant": "new"}})

        def restart(config, output, fresh_boot):
            if output["instance"] == "test":
                raise RuntimeError("fixture reload failure")

        with patch.object(controller, "restart_bootstrap", restart):
            with self.assertRaisesRegex(RuntimeError, "Reconciliation incomplete"):
                controller.reconcile(config, self.store)
        self.assertNotIn("pr-1", self.cloud.managed)
        self.assertEqual(self.store.state["environments"]["pr-1"]["status"], "destroyed")
        self.assertNotIn(("bootstrap", "pr-1", False), self.calls)
        self.assertNotIn(("unseal", "pr-1"), self.calls)
        self.assertTrue(self.store.state["environments"]["test"]["gcip_pending"])

    def test_resumed_and_replaced_hosts_wait_for_their_boot_script(self):
        self.request()
        self.reconcile()
        self.request(action="stop")
        self.reconcile()
        self.request(action="resume")
        self.reconcile()
        self.assertEqual(self.calls.count(("bootstrap", "test", True)), 2)
        self.request(action="resume", mode="normal")
        self.reconcile()
        self.assertEqual(self.calls.count(("bootstrap", "test", True)), 3)

    def test_missing_vm_is_removed_from_intent_before_any_other_apply(self):
        self.request()
        self.reconcile()
        self.request("pr-1")
        self.reconcile()
        self.cloud.status["test"] = "MISSING"
        self.request("pr-1", "stop")
        self.cloud.plans.clear()
        self.reconcile()
        self.assertTrue(self.cloud.plans)
        self.assertTrue(
            all(not plan[0]["test"]["vm_present"] for plan in self.cloud.plans)
        )

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
        self.assertIn(("bootstrap", "pr-1", True), self.calls)
        self.assertEqual(self.calls.count(("bootstrap", "pr-1", False)), 1)

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
        self.assertLess(self.calls.index(("seal", "develop")), self.calls.index(("unseal", "develop")))
        self.assertTrue(self.cloud.managed["develop"]["running"])
        self.assertTrue(self.cloud.managed["develop"]["published"])
        self.assertEqual(self.cloud.managed["develop"]["release_sha"], SHA)
        self.assertFalse(any(call[0] in {"bootstrap", "start", "stop"} for call in self.calls))

    def test_pending_build_reloads_changed_broker_settings_for_the_existing_release(self):
        self.config["develop_branch"] = "develop/0.1.0"
        initial = {"develop": {"enabled": True, "image": "initial", "signing_version": "1"}}
        self.config["credential_brokers"] = deepcopy(initial)
        outputs = self.cloud.outputs
        def managed_outputs():
            values = outputs()
            for identity, output in values.items():
                broker = self.cloud.brokers.get(identity, {})
                output["provider_credentials"] = {
                    "store": {"environment_id": identity},
                    "broker": {"endpoint": "https://broker.run.app/api/v1", "kid": broker.get("signing_version", "1")} if broker.get("enabled") else None,
                }
            return values
        with patch.object(self.cloud, "outputs", side_effect=managed_outputs):
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
                    self.assertEqual([call for call in self.calls if call[0] in {"seal", "bootstrap", "health", "unseal"}], [("seal", "develop"), ("bootstrap", "develop", False), ("health", "develop"), ("unseal", "develop")])
                    entry = self.store.state["environments"]["develop"]
                    self.assertEqual(entry["status"], "awaiting_build")
                    self.assertEqual(entry["provider_credentials"], self.cloud.outputs()["develop"]["provider_credentials"])
                    self.assertEqual(self.cloud.managed["develop"]["release_sha"], SHA)
                    self.assertTrue(self.cloud.managed["develop"]["published"])
                    self.assertTrue(self.cloud.managed["develop"]["running"])
                    self.assertFalse(any(call[0] in {"start", "stop"} for call in self.calls))
                    self.calls.clear()
                    self.cloud.plans.clear()
                    self.reconcile()
                    self.assertFalse(any(call[0] in {"bootstrap", "start", "stop"} for call in self.calls))
                    self.assertEqual(self.cloud.plans, [])

    def test_pending_build_recovers_a_broker_apply_completed_by_an_interrupted_controller(self):
        self.config["develop_branch"] = "develop/0.1.0"
        self.config["credential_brokers"] = {"develop": {"enabled": True, "image": "initial"}}
        outputs = self.cloud.outputs
        def managed_outputs():
            values = outputs()
            for identity, output in values.items():
                output["provider_credentials"] = {
                    "store": {"environment_id": identity},
                    "broker": {"endpoint": "https://broker.run.app/api/v1", "kid": "version-1"} if self.cloud.brokers.get(identity, {}).get("enabled") else None,
                }
            return values
        with patch.object(self.cloud, "outputs", side_effect=managed_outputs):
            self.request("develop", source_ref="develop/0.1.0")
            self.reconcile()
            self.request("develop", action="update", sha="b" * 40, source_ref="develop/0.1.0")["release"] = None
            self.config["credential_brokers"] = {}
            # The prior controller applied broker metadata, then exited before
            # reloading the old release. Terraform already matches desired intent.
            self.cloud.apply(self.cloud.managed)
            self.calls.clear()
            self.cloud.plans.clear()
            self.reconcile()
            self.assertEqual([call for call in self.calls if call[0] in {"seal", "bootstrap", "health", "unseal"}], [("seal", "develop"), ("bootstrap", "develop", False), ("health", "develop"), ("unseal", "develop")])
            entry = self.store.state["environments"]["develop"]
            self.assertEqual(entry["status"], "awaiting_build")
            self.assertIsNone(entry["provider_credentials"]["broker"])
            self.assertEqual(self.cloud.managed["develop"]["release_sha"], SHA)
            self.assertEqual(self.cloud.plans, [])
            self.assertFalse(any(call[0] in {"start", "stop"} for call in self.calls))

    def test_pending_build_broker_settings_reload_failure_keeps_admission_gated(self):
        self.config["develop_branch"] = "develop/0.1.0"
        self.config["credential_brokers"] = {"develop": {"enabled": True, "image": "initial"}}
        self.request("develop", source_ref="develop/0.1.0")
        self.reconcile()
        self.request("develop", action="update", sha="b" * 40, source_ref="develop/0.1.0")["release"] = None
        self.config["credential_brokers"] = {}
        outputs = self.cloud.outputs
        def managed_outputs():
            values = outputs()
            values["develop"]["provider_credentials"] = {"store": {"environment_id": "develop"}, "broker": None}
            return values
        self.calls.clear()
        with patch.object(self.cloud, "outputs", side_effect=managed_outputs), patch.object(controller, "restart_bootstrap", side_effect=RuntimeError("reload unavailable")), self.assertRaisesRegex(RuntimeError, "Reconciliation incomplete"):
            self.reconcile()
        self.assertIn(("gate", "develop"), self.calls)
        self.assertNotIn(("unseal", "develop"), self.calls)
        self.assertFalse(self.cloud.managed["develop"]["published"])
        entry = self.store.state["environments"]["develop"]
        self.assertEqual(entry["failed_deployment_generation"], entry["generation"])
        self.assertFalse(any(call[0] in {"start", "stop"} for call in self.calls))

    def test_declined_idle_stop_restores_broker_presealed_healthy_release(self):
        self.config["credential_brokers"] = {"test": {"enabled": True, "image": "initial"}}
        self.request()
        self.reconcile()
        self.config["credential_brokers"]["test"]["image"] = "rotated"
        self.idle = True
        self.calls.clear()
        def declined_idle_seal(config, output, action, force=False):
            result = self.host(config, output, action, force)
            return {"sealed": False} if action == "seal-idle" else result
        with patch.object(controller, "host", side_effect=declined_idle_seal):
            self.reconcile()
        self.assertEqual(self.calls, [("seal", "test"), ("observe", "test"), ("seal-idle", "test"), ("unseal", "test")])
        self.assertTrue(self.cloud.managed["test"]["running"])
        self.assertTrue(self.cloud.managed["test"]["published"])

    def test_broker_change_does_not_unseal_a_deliberately_failed_release(self):
        self.config["credential_brokers"] = {"test": {"enabled": True, "image": "initial"}}
        self.request()
        self.reconcile()
        self.request(action="resume", force=True)
        with patch.object(controller, "bundle", side_effect=RuntimeError("bundle unavailable")), self.assertRaises(RuntimeError):
            self.reconcile()
        self.assertFalse(self.cloud.managed["test"]["published"])
        self.config["credential_brokers"]["test"]["image"] = "rotated"
        self.calls.clear()
        self.reconcile()
        self.assertNotIn(("unseal", "test"), self.calls)
        self.assertFalse(self.cloud.managed["test"]["published"])
        self.assertFalse(any(call[0] in {"bootstrap", "start", "stop"} for call in self.calls))

    def test_pending_build_keeps_observing_old_release_and_stops_when_idle(self):
        self.request("pr-1")
        self.reconcile()
        entry = self.request("pr-1", "update", sha="b" * 40)
        entry["release"] = None
        self.calls.clear()
        self.busy = True
        self.idle = True
        self.reconcile()
        self.assertEqual(self.cloud.status["pr-1"], "RUNNING")
        self.assertEqual(
            self.store.state["environments"]["pr-1"]["status"], "awaiting_build"
        )
        self.assertIn(("observe", "pr-1"), self.calls)
        self.busy = False
        self.reconcile()
        self.assertEqual(self.cloud.status["pr-1"], "TERMINATED")
        self.assertEqual(self.cloud.managed["pr-1"]["release_sha"], SHA)
        self.assertFalse(self.cloud.managed["pr-1"]["published"])
        self.assertEqual(self.store.state["environments"]["pr-1"]["desired"], "stopped")
        self.assertFalse(any(call[0] == "bootstrap" for call in self.calls))
        self.reconcile()
        self.assertEqual(self.cloud.status["pr-1"], "TERMINATED")

    def test_failed_bootstrap_waits_for_explicit_retry_and_keeps_data(self):
        self.request()
        with patch.object(
            controller,
            "restart_bootstrap",
            side_effect=RuntimeError("fixture boot failure"),
        ):
            with self.assertRaises(RuntimeError):
                self.reconcile()
        self.assertFalse(self.cloud.managed["test"]["published"])
        self.calls.clear()
        self.reconcile()
        self.assertFalse(any(call[0] == "bootstrap" for call in self.calls))
        self.request(action="resume", force=True)
        self.reconcile()
        self.assertEqual(self.store.state["environments"]["test"]["status"], "ready")
        self.assertIn(("bootstrap", "test", False), self.calls)
        self.assertFalse(any(retiring for _, retiring in self.cloud.plans))

    def test_stop_waits_for_work_and_destroy_only_retires_its_owned_environment(self):
        self.request()
        self.reconcile()
        self.request("pr-1")
        self.reconcile()
        self.request(action="stop")
        self.busy = True
        self.reconcile()
        self.assertEqual(self.cloud.status["test"], "RUNNING")
        self.request(action="destroy")
        self.reconcile()
        self.assertNotIn("test", self.cloud.managed)
        self.assertIn("pr-1", self.cloud.managed)
        self.assertEqual(
            [retiring for _, retiring in self.cloud.plans if retiring], [{"test"}]
        )

    def test_deadline_fences_an_incomplete_bootstrap_and_releases_the_lock(self):
        self.request()
        with patch.object(
            controller,
            "restart_bootstrap",
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
        self.assertFalse(any(call[0] == "bootstrap" for call in self.calls))

    def test_closed_pr_cleans_up_even_when_another_host_is_unhealthy(self):
        self.request()
        self.reconcile()
        self.request("pr-1")
        self.reconcile()
        self.closed.add("pr-1")
        with patch.object(
            controller, "host", side_effect=RuntimeError("fixture unavailable")
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
        original = self.host

        def racing_host(config, output, action, force=False):
            if action == "seal-idle":
                self.request(action="resume")
            return original(config, output, action, force)

        with patch.object(controller, "host", racing_host):
            with self.assertRaises(RuntimeError):
                self.reconcile()
        self.assertNotIn(("stop", "test"), self.calls)
        self.assertEqual(self.store.state["environments"]["test"]["desired"], "running")

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
                self.assertEqual(self.cloud.status["test"], "TERMINATED")
                self.assertNotIn(("start", "test"), self.calls)
                self.assertFalse(any(call[0] == "bootstrap" for call in self.calls))

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


class BootstrapTests(unittest.TestCase):
    def test_initial_boot_is_waited_out_and_only_failure_or_update_restarts(self):
        for fresh_boot, failed, expected_restarts in (
            (True, False, 0),
            (True, True, 1),
            (False, False, 1),
        ):
            with self.subTest(fresh_boot=fresh_boot, failed=failed):
                observations = iter(
                    [
                        b"ActiveState=inactive\nResult=success\nExecMainStatus=0\nExecMainStartTimestampMonotonic=0\nExecMainExitTimestampMonotonic=0\n",
                        b"ActiveState=activating\nResult=success\nExecMainStatus=0\nExecMainStartTimestampMonotonic=1\nExecMainExitTimestampMonotonic=0\n",
                        (
                            b"ActiveState=failed\nResult=exit-code\nExecMainStatus=1\n"
                            if failed
                            else b"ActiveState=inactive\nResult=success\nExecMainStatus=0\n"
                        )
                        + b"ExecMainStartTimestampMonotonic=1\nExecMainExitTimestampMonotonic=2\n",
                    ]
                )
                finished = False
                restarts = []

                def run(*args, observations=observations, restarts=restarts, **kwargs):
                    nonlocal finished
                    remote = args[args.index("--command") + 1]
                    if remote == "true":
                        return b""
                    if "show" in remote:
                        state = next(observations)
                        finished = b"ExecMainExitTimestampMonotonic=2" in state
                        return state
                    if "restart" in remote:
                        self.assertTrue(
                            finished,
                            "must not interrupt an active or pending bootstrap",
                        )
                        restarts.append(remote)
                        return b""
                    self.fail(f"unexpected remote operation: {remote}")

                with (
                    patch.object(controller, "run", run),
                    patch.object(controller.time, "sleep"),
                ):
                    controller.restart_bootstrap(
                        {"project_id": "fixture"},
                        {"instance": "fixture", "zone": "us-central1-a"},
                        fresh_boot,
                    )
                self.assertEqual(len(restarts), expected_restarts)

    def test_unfinished_bootstrap_times_out_without_replaying(self):
        active = b"ActiveState=activating\nResult=success\nExecMainStatus=0\nExecMainStartTimestampMonotonic=1\nExecMainExitTimestampMonotonic=0\n"

        def run(*args, **kwargs):
            remote = args[args.index("--command") + 1]
            self.assertNotIn("restart", remote)
            return active

        with (
            patch.object(controller, "run", run),
            patch.object(controller.time, "sleep"),
        ):
            with self.assertRaisesRegex(RuntimeError, "startup script did not finish"):
                controller.restart_bootstrap(
                    {"project_id": "fixture"},
                    {"instance": "fixture", "zone": "us-central1-a"},
                    True,
                )


if __name__ == "__main__":
    unittest.main()
