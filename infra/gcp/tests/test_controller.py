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
import controller
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

    def broker_configuration_changed(self, store, environments):
        return self.brokers != self.broker_configuration(environments)

    def broker_configuration(self, environments):
        return {
            key: dict(value, enabled=value.get("enabled") or False)
            for key, value in self.configuration.get("credential_brokers", {}).items()
            if key in environments
        }

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
        self.brokers = deepcopy(self.broker_configuration(managed))

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
        self.config = deepcopy(CONFIG)
        self.sequence = 0
        self.calls = []
        self.busy = False
        self.idle = False
        self.closed = set()
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
        controller.reconcile(self.config, self.store)

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
                        call[0] in {"bootstrap", "start", "stop", "seal"}
                        for call in self.calls
                    )
                )
                self.cloud.plans.clear()
                self.reconcile()
                self.assertEqual(self.cloud.plans, [])

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
