"""Plan review must enforce authorization even for drift after preflight."""

import json
import io
import os
from pathlib import Path
import sys
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import Mock, patch
from urllib.error import HTTPError

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "control"))
from cloud import (
    Store,
    Terraform,
    OperationDeadline,
    operation_budget,
    run,
    bounded_timeout,
    retire_provider_credentials,
)


class LockTests(unittest.TestCase):
    def test_operation_budget_stops_work_and_leaves_time_to_release_its_lock(self):
        store = Store("fixture")
        released = []

        def release(method, path):
            self.assertEqual(method, "DELETE")
            self.assertIn("ifGenerationMatch=owned-generation", path)
            self.assertGreater(bounded_timeout(60), 1)
            released.append(path)

        with (
            patch.object(store, "write", return_value="owned-generation"),
            patch.object(store, "call", side_effect=release),
        ):
            with self.assertRaises(OperationDeadline):
                with operation_budget(0.1), store.lock():
                    run(sys.executable, "-c", "import time; time.sleep(0.3)", timeout=5)
                    self.fail("the operation exceeded its controller budget")
        self.assertEqual(len(released), 1)

    def test_busy_lock_waits_without_stealing_then_releases_only_its_generation(self):
        store = Store("fixture")
        with (
            patch.object(
                store,
                "write",
                side_effect=[HTTPError("", 412, "busy", {}, None), "owned-generation"],
            ) as write,
            patch.object(store, "call") as call,
            patch("cloud.time.monotonic", return_value=0),
            patch("cloud.time.sleep") as sleep,
        ):
            with store.lock(wait_seconds=10):
                call.assert_not_called()
            sleep.assert_called_once_with(5)
            self.assertEqual(
                [item.args[2] for item in write.call_args_list], ["0", "0"]
            )
            call.assert_called_once_with(
                "DELETE",
                "storage/v1/b/fixture/o/lifecycle%2Fapply.lock?ifGenerationMatch=owned-generation",
            )

    def test_expired_wait_never_deletes_someone_elses_lock(self):
        store = Store("fixture")
        with (
            patch.object(
                store, "write", side_effect=HTTPError("", 412, "busy", {}, None)
            ),
            patch.object(store, "call") as call,
            patch("cloud.time.monotonic", side_effect=[0, 10]),
            patch("cloud.time.sleep") as sleep,
        ):
            with self.assertRaisesRegex(RuntimeError, "lifecycle lock is busy"):
                with store.lock(wait_seconds=10):
                    self.fail("must not enter an unowned critical section")
            call.assert_not_called()
            sleep.assert_not_called()


class ProviderCredentialRetirementTests(unittest.TestCase):
    def retire(self, responses):
        calls = []

        def api(request, **kwargs):
            calls.append((request.get_method(), request.full_url))
            self.assertEqual(request.headers["Authorization"], "Bearer fixture-token")
            self.assertLessEqual(kwargs["timeout"], 60)
            expected, result = responses.pop(0)
            self.assertEqual(request.get_method(), expected)
            if isinstance(result, BaseException):
                raise result
            return io.BytesIO(json.dumps(result).encode())

        with (
            patch("cloud.run", side_effect=[b"123456789012\n", b"fixture-token\n"]) as cli,
            patch("cloud.urllib.request.urlopen", side_effect=api),
        ):
            count = retire_provider_credentials({"byok_project_id": "fixture-byok"}, "pr-12")
        self.assertEqual(cli.call_count, 2)
        self.assertFalse(responses)
        return count, calls

    def test_all_pages_are_collected_before_exact_prefix_deletion(self):
        prefix = "projects/123456789012/secrets/"
        own = [prefix + "aidash-pr-12-cred-a", prefix + "aidash-pr-12-cred-b"]
        other = [prefix + "aidash-pr-120-cred-a", prefix + "aidash-test-cred-a", prefix + "unrelated"]
        count, calls = self.retire([
            ("GET", {"secrets": [{"name": name} for name in [own[0], *other]], "nextPageToken": "next/page"}),
            ("GET", {"secrets": [{"name": name} for name in own]}),
            ("DELETE", {}), ("DELETE", {}),
            ("GET", {"secrets": [{"name": name} for name in other]}),
        ])
        self.assertEqual(count, 2)
        self.assertEqual([method for method, _ in calls], ["GET", "GET", "DELETE", "DELETE", "GET"])
        self.assertIn("pageToken=next%2Fpage", calls[1][1])
        self.assertEqual([url.rsplit("/", 1)[1] for method, url in calls if method == "DELETE"], ["aidash-pr-12-cred-a", "aidash-pr-12-cred-b"])
        self.assertTrue(all("/versions/" not in url for _, url in calls))

    def test_already_deleted_secrets_are_idempotent(self):
        name = "projects/123456789012/secrets/aidash-pr-12-cred-a"
        count, _ = self.retire([
            ("GET", {"secrets": [{"name": name}]}),
            ("DELETE", HTTPError("", 404, "gone", {}, None)),
            ("GET", {}),
        ])
        self.assertEqual(count, 1)
        self.assertEqual(self.retire([("GET", {}), ("GET", {})])[0], 0)

    def test_delete_failure_and_remaining_inventory_block_retirement(self):
        name = "projects/123456789012/secrets/aidash-pr-12-cred-a"
        with self.assertRaises(HTTPError):
            self.retire([("GET", {"secrets": [{"name": name}]}), ("DELETE", HTTPError("", 403, "denied", {}, None))])
        with self.assertRaisesRegex(RuntimeError, "incomplete"):
            self.retire([("GET", {"secrets": [{"name": name}]}), ("DELETE", {}), ("GET", {"secrets": [{"name": name}]})])

    def test_inventory_failure_unexpected_project_and_repeated_page_fail_closed(self):
        with self.assertRaises(HTTPError):
            self.retire([("GET", HTTPError("", 403, "denied", {}, None))])
        with self.assertRaisesRegex(RuntimeError, "unexpected"):
            self.retire([("GET", {"secrets": [{"name": "projects/999/secrets/aidash-pr-12-cred-a"}]})])
        with self.assertRaisesRegex(RuntimeError, "pagination"):
            self.retire([("GET", {"nextPageToken": "same"}), ("GET", {"nextPageToken": "same"})])

    def test_legacy_configuration_and_invalid_environment_make_no_cloud_calls(self):
        with patch("cloud.run") as cli:
            self.assertEqual(retire_provider_credentials({}, "test"), 0)
            with self.assertRaises(ValueError):
                retire_provider_credentials({"byok_project_id": "fixture-byok"}, "pr-12/../../other")
        cli.assert_not_called()


class AppliedProviderProjectTests(unittest.TestCase):
    def configuration(self, applied, configured, managed=True, environment_project=None):
        terraform = Terraform.__new__(Terraform)
        terraform.configuration = {"byok_project_id": configured}
        state = {"outputs": {
            "managed_configuration": {"value": {"test": {}} if managed else {}},
            "byok_project_id": {"value": applied},
            "environments": {"value": {
                "test": {"byok_project_id": applied if environment_project is None else environment_project}
            } if managed else {}},
        }}
        store = Mock()
        store.read.return_value = (state, "1")
        return terraform.configuration_in_state(store)

    def test_applied_project_cannot_be_removed_or_replaced_before_retirement(self):
        for configured in ["", "new-byok"]:
            with self.subTest(configured=configured):
                with self.assertRaisesRegex(RuntimeError, "Restore the applied BYOK project"):
                    self.configuration("old-byok", configured)

    def test_per_environment_output_preserves_project_when_root_output_is_missing(self):
        with self.assertRaisesRegex(RuntimeError, "Restore the applied BYOK project"):
            self.configuration("", "new-byok", environment_project="old-byok")

    def test_same_project_and_legacy_enablement_remain_supported(self):
        self.assertEqual(self.configuration("old-byok", "old-byok"), {"test": {}})
        self.assertEqual(self.configuration("", "new-byok"), {"test": {}})

    def test_project_may_change_after_all_environment_prefixes_are_retired(self):
        for configured in ["", "new-byok"]:
            with self.subTest(configured=configured):
                self.assertEqual(self.configuration("old-byok", configured, managed=False), {})


class PlanTests(unittest.TestCase):
    def test_persisted_broker_intent_detects_rotation_disable_and_legacy_state(self):
        terraform = Terraform.__new__(Terraform)
        broker = {"image": "digest", "enabled": True}
        terraform.configuration = {
            "credential_brokers": {"test": broker, "retired": broker}
        }
        environments = {"test": {"kind": "test"}}
        state = {"outputs": {"managed_credential_brokers": {"value": {"test": broker}}}}
        store = Mock()
        store.read.return_value = (state, "1")
        self.assertFalse(terraform.broker_configuration_changed(store, environments))
        terraform.configuration["credential_brokers"]["test"] = dict(
            broker, image="new-digest"
        )
        self.assertTrue(terraform.broker_configuration_changed(store, environments))
        terraform.configuration["credential_brokers"] = {}
        self.assertTrue(terraform.broker_configuration_changed(store, environments))
        state["outputs"]["managed_credential_brokers"]["value"] = {}
        self.assertFalse(terraform.broker_configuration_changed(store, environments))
        del state["outputs"]["managed_credential_brokers"]
        self.assertTrue(terraform.broker_configuration_changed(store, environments))
        terraform.configuration["credential_brokers"] = {"test": {"image": "digest"}}
        state["outputs"]["managed_credential_brokers"] = {
            "value": {"test": {"image": "digest", "enabled": False}}
        }
        self.assertFalse(terraform.broker_configuration_changed(store, environments))

    def test_broker_opt_in_is_forwarded_only_for_environments_in_current_intent(self):
        with TemporaryDirectory() as directory:
            terraform = Terraform.__new__(Terraform)
            terraform.root = Path(directory)
            terraform.configuration = dict.fromkeys(
                ("project_id", "byok_project_id", "cloudflare_zone_id", "release_bucket", "deploy_service_account", "domain"), "fixture"
            )
            broker = {"enabled": True, "byok_project_id": "byok", "secret_prefix": "aidash-test-cred-", "broker_service_account_email": "aidash-test-broker@fixture.iam.gserviceaccount.com", "image": "digest"}
            terraform.configuration["credential_brokers"] = {"test": broker, "retired": broker}
            variables = []

            def command(*args, **kwargs):
                if "plan" in args:
                    variables.append(json.loads((terraform.root / "controller.auto.tfvars.json").read_text()))
                return b'{"resource_changes":[]}'

            with patch("cloud.run", side_effect=command):
                terraform.apply({"test": {"kind": "test"}})
            self.assertEqual(variables[0]["credential_brokers"], {"test": dict(broker, signing_version="1", verification_versions=["1"])})

    def test_persisted_version_sets_and_defaults_do_not_cause_repeated_apply(self):
        terraform = Terraform.__new__(Terraform)
        terraform.configuration = {"credential_brokers": {"test": {
            "enabled": True, "image": "digest", "signing_version": "2",
            "verification_versions": ["2", "1", "2"],
        }}}
        store = Mock()
        store.read.return_value = ({"outputs": {"managed_credential_brokers": {"value": {"test": {
            "enabled": True, "image": "digest", "signing_version": "2",
            "verification_versions": ["1", "2"],
        }}}}}, "1")
        self.assertFalse(terraform.broker_configuration_changed(store, {"test": {}}))
        terraform.configuration["credential_brokers"]["test"]["signing_version"] = "1"
        self.assertTrue(terraform.broker_configuration_changed(store, {"test": {}}))

    def test_legacy_configuration_applies_without_byok_resources(self):
        with TemporaryDirectory() as directory:
            terraform = Terraform.__new__(Terraform)
            terraform.root = Path(directory)
            terraform.configuration = dict.fromkeys(
                ("project_id", "cloudflare_zone_id", "release_bucket",
                 "deploy_service_account", "domain"), "fixture"
            )

            def command(*args, **kwargs):
                variables = json.loads(
                    (terraform.root / "controller.auto.tfvars.json").read_text()
                )
                self.assertEqual(variables["byok_project_id"], "")
                self.assertEqual(variables["project_id"], "fixture")
                return b'{"resource_changes":[]}'

            with patch("cloud.run", side_effect=command) as run:
                terraform.apply({})
            self.assertTrue(any("apply" in call.args for call in run.call_args_list))

    def apply(self, resource_type, actions, address=None, before=None, after=None, **authorization):
        with TemporaryDirectory() as directory:
            terraform = Terraform.__new__(Terraform)
            terraform.root = Path(directory)
            terraform.configuration = dict.fromkeys(
                (
                    "project_id",
                    "byok_project_id",
                    "cloudflare_zone_id",
                    "deploy_service_account",
                    "domain",
                ),
                "fixture",
            )
            plan = {
                "resource_changes": [
                    {
                        "address": address or f"{resource_type}.fixture",
                        "type": resource_type,
                        "change": {"actions": actions, "before": before, "after": after},
                    }
                ]
            }
            with patch("cloud.run", return_value=json.dumps(plan).encode()) as command:
                try:
                    terraform.apply({}, **authorization)
                except RuntimeError:
                    self.assertFalse(any("apply" in call.args for call in command.call_args_list))
                    raise
                finally:
                    self.assertFalse(
                        (terraform.root / "controller.auto.tfvars.json").exists()
                    )
                    self.assertFalse((terraform.root / "controller.tfplan").exists())
            self.assertTrue(
                any("apply" in call.args for call in command.call_args_list)
            )

    def pool(self, actions, before, after, identity="test", **authorization):
        address = (
            f'module.environment["{identity}"].google_container_node_pool.environment'
            if identity else "google_container_node_pool.system"
        )
        self.apply(
            "google_container_node_pool", actions, address,
            None if before is None else {"node_count": before},
            None if after is None else {"node_count": after},
            **authorization,
        )

    def test_refresh_cannot_start_a_node_pool_without_a_start_authorization(self):
        for actions, before in [(["update"], 0), (["create"], None), (["delete", "create"], 0)]:
            with self.subTest(actions=actions):
                with self.assertRaisesRegex(RuntimeError, "start authorization"):
                    self.pool(actions, before, 1, starting={"pr-1"}, retiring={"test"})
                self.pool(actions, before, 1, starting={"test"})
        # Scaling down, staying stopped and the shared system pool need no start authorization.
        self.pool(["update"], 1, 0)
        self.pool(["create"], None, 0)
        self.pool(["update"], 0, 1, identity=None)

    def test_node_pool_deletion_requires_retirement_of_its_owner(self):
        with self.assertRaisesRegex(RuntimeError, "explicit retirement"):
            self.pool(["delete"], 1, None, retiring={"pr-1"})
        with self.assertRaisesRegex(RuntimeError, "explicit retirement"):
            self.pool(["delete", "create"], 1, 1, retiring={"pr-1"})
        with self.assertRaisesRegex(RuntimeError, "explicit retirement"):
            self.pool(["delete"], 1, None, identity=None, retiring={"test"})
        self.pool(["delete"], 0, None, retiring={"test"})
        # A replacement (Spot toggle) is part of an explicit start.
        self.pool(["delete", "create"], 1, 1, starting={"test"})

    def test_plan_fence_runs_before_apply_and_can_abort_it(self):
        seen = []

        def reject(plan):
            seen.append(plan["resource_changes"][0]["type"])
            raise RuntimeError("fixture GCIP fence refused")

        with self.assertRaisesRegex(RuntimeError, "GCIP fence refused"):
            self.apply("google_identity_platform_tenant", ["delete", "create"], before_apply=reject)
        self.assertEqual(seen, ["google_identity_platform_tenant"])

    def test_unconfigured_workflow_secret_is_an_empty_gcip_input(self):
        with patch.dict(os.environ, {"AIDASH_GCIP_IDP_SECRETS": ""}):
            self.apply("google_container_node_pool", ["no-op"])

    def test_shared_cluster_and_preview_disk_are_never_deleted(self):
        for kind, message in [
            ("google_container_cluster", "shared cluster"),
            ("google_compute_disk", "preview TLS disk"),
        ]:
            for actions in [["delete"], ["delete", "create"], ["create", "delete"]]:
                with self.subTest(kind=kind, actions=actions):
                    with self.assertRaisesRegex(RuntimeError, message):
                        self.apply(kind, actions, retiring={"test"}, starting={"test"})
            self.apply(kind, ["update"])

    def test_environment_automation_never_destroys_signing_keys_or_versions(self):
        for kind in ["google_kms_key_ring", "google_kms_crypto_key", "google_kms_crypto_key_version"]:
            for actions in [["delete"], ["delete", "create"]]:
                with self.subTest(kind=kind, actions=actions):
                    with self.assertRaisesRegex(RuntimeError, "transfer draft key state to bootstrap"):
                        self.apply(kind, actions, retiring={"test"})


if __name__ == "__main__":
    unittest.main()
