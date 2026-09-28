"""Exercise complete reconciliation with cloud boundaries replaced, not the policy."""

from contextlib import ExitStack, nullcontext
from copy import deepcopy
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "control"))
import controller
from policy import transition


SHA = "a" * 40
CONFIG = {"repository": "kent8192/aidash"}


class MemoryStore:
    def __init__(self):
        self.state = {"environments": {}}

    def read(self, key):
        return deepcopy(self.state), "1"

    def mutate(self, callback):
        self.state, value = callback(deepcopy(self.state))
        return value

    def lock(self):
        return nullcontext()


class CloudFixture:
    def __init__(self):
        self.managed = {}
        self.status = {}
        self.plans = []

    def configuration_in_state(self, store):
        return deepcopy(self.managed)

    def apply(self, managed, retiring=(), starting=()):
        self.plans.append((deepcopy(managed), set(retiring)))
        for identity, value in managed.items():
            if value.get("vm_present", True) and (
                identity not in self.managed
                or not self.managed[identity].get("vm_present", True)
            ):
                self.status[identity] = "RUNNING"
        self.managed = deepcopy(managed)

    def outputs(self):
        return {
            key: dict(
                instance=key,
                zone="us-central1-a",
                hostname="test.aidash.run",
                runtime_secret=key,
            )
            for key in self.managed
        }


class ReconcileTests(unittest.TestCase):
    def setUp(self):
        self.store = MemoryStore()
        self.cloud = CloudFixture()
        self.sequence = 0
        self.calls = []
        self.busy = False
        self.idle = False
        self.closed = set()
        self.context = ExitStack()
        self.addCleanup(self.context.close)
        replacements = {
            "Terraform": lambda *args: self.cloud,
            "instance_status": lambda config, output: self.cloud.status.get(
                output["instance"], "MISSING"
            ),
            "host": self.host,
            "power": self.power,
            "bundle": lambda *args: ("bundles/" + "b" * 64 + ".tar.gz", "b" * 64),
            "ci_success": lambda *args: None,
            "provision_secret": lambda *args: None,
            "restart_bootstrap": lambda *args: self.calls.append(
                ("bootstrap", args[1]["instance"])
            ),
            "public_health": lambda *args: None,
            "github": self.github,
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

    def host(self, config, output, action, force=False):
        self.calls.append((action, output["instance"]))
        if action.startswith("seal"):
            return {"sealed": not self.busy}
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

    def reconcile(self):
        controller.reconcile(CONFIG, self.store)

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
        self.assertNotIn(("bootstrap", "test"), self.calls)
        self.request(action="resume", force=True)
        self.reconcile()
        self.assertEqual(self.store.state["environments"]["test"]["status"], "ready")
        self.assertIn(("bootstrap", "test"), self.calls)
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


if __name__ == "__main__":
    unittest.main()
