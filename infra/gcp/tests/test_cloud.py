"""Plan review must enforce authorization even for drift after preflight."""

import json
import io
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
            self.assertEqual(variables[0]["credential_brokers"], {"test": broker})

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

    def apply(self, resource_type, actions, **authorization):
        with TemporaryDirectory() as directory:
            terraform = Terraform.__new__(Terraform)
            terraform.root = Path(directory)
            terraform.configuration = dict.fromkeys(
                (
                    "project_id",
                    "byok_project_id",
                    "cloudflare_zone_id",
                    "release_bucket",
                    "deploy_service_account",
                    "domain",
                ),
                "fixture",
            )
            plan = {
                "resource_changes": [
                    {
                        "type": resource_type,
                        "change": {
                            "actions": actions,
                            "before": {"labels": {"environment": "test"}},
                            "after": {"labels": {"environment": "test"}},
                        },
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

    def test_refresh_cannot_recreate_a_vm_deleted_after_preflight(self):
        with self.assertRaisesRegex(RuntimeError, "power authorization"):
            self.apply("google_compute_instance", ["create"])
        with self.assertRaises(RuntimeError):
            self.apply(
                "google_compute_instance", ["delete", "create"], starting={"pr-1"}
            )
        self.apply("google_compute_instance", ["create"], starting={"test"})

    def test_disk_replacement_requires_explicit_retirement_for_that_owner(self):
        with self.assertRaisesRegex(RuntimeError, "retained disk"):
            self.apply("google_compute_disk", ["delete", "create"])
        with self.assertRaises(RuntimeError):
            self.apply("google_compute_disk", ["delete"], retiring={"pr-1"})
        self.apply("google_compute_disk", ["delete"], retiring={"test"})

    def test_environment_automation_never_destroys_signing_keys_or_versions(self):
        for kind in ["google_kms_key_ring", "google_kms_crypto_key", "google_kms_crypto_key_version"]:
            for actions in [["delete"], ["delete", "create"]]:
                with self.subTest(kind=kind, actions=actions):
                    with self.assertRaisesRegex(RuntimeError, "transfer draft key state to bootstrap"):
                        self.apply(kind, actions, retiring={"test"})


if __name__ == "__main__":
    unittest.main()
