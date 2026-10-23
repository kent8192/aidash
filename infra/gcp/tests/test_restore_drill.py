import copy
import json
import urllib.error
import importlib.util
import pathlib
import tempfile
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location("restore_drill", pathlib.Path(__file__).parents[1] / "restore_drill.py")
drill = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(drill)
NAME = "projects/aidash-production/locations/us-central1/backupPlans/aidash-production-pre-update/backups/pre-update-g3-012345abcdef"
SETTINGS = {"billing_account": "000000-000000-000000", "folder_id": "123", "region": "us-central1", "client_cidr": "192.0.2.1/32", "namespace": "aidash-production", "ledger_claim": "memory-recovery", "source_cluster": "projects/aidash-production/locations/us-central1/clusters/production"}
BACKUP = {"name": NAME, "state": "SUCCEEDED", "containsVolumeData": True, "volumeCount": 1, "resourceCount": 4, "selectedApplications": {"namespacedNames": [{"namespace": "aidash-production", "name": "aidash"}]}, "clusterMetadata": {"cluster": SETTINGS["source_cluster"]}}
VOLUMES = [{"sourcePvc": {"namespace": "aidash-production", "name": "postgres"}, "state": "SUCCEEDED"}]


class RestoreDrillTests(unittest.TestCase):
    def test_baseline_refuses_later_writers_wrong_cluster_and_incomplete_volumes(self):
        drill.validate_baseline({"pre_update_backup": NAME}, SETTINGS, BACKUP, VOLUMES)
        for mutation in ["writers", "cluster", "scope", "permissive", "volumes", "ledger"]:
            with self.subTest(mutation=mutation):
                entry, backup, volumes = {"pre_update_backup": NAME}, copy.deepcopy(BACKUP), copy.deepcopy(VOLUMES)
                if mutation == "writers":
                    entry["writers_started"] = True
                elif mutation == "cluster":
                    backup["clusterMetadata"]["cluster"] = "another-cluster"
                elif mutation == "scope":
                    backup["allNamespaces"] = True
                elif mutation == "permissive":
                    backup["permissiveMode"] = True
                elif mutation == "volumes":
                    volumes = []
                else:
                    volumes[0]["sourcePvc"]["name"] = SETTINGS["ledger_claim"]
                with self.assertRaises(ValueError):
                    drill.validate_baseline(entry, SETTINGS, backup, volumes)

    def test_volume_inventory_reads_every_page_before_validating_the_ledger_exclusion(self):
        pages = [{"volumeBackups": VOLUMES, "nextPageToken": "next"}, {"volumeBackups": [{"sourcePvc": {"namespace": "aidash-production", "name": "memory-recovery"}, "state": "SUCCEEDED"}]}]
        with patch.object(drill, "call", side_effect=pages) as cloud:
            volumes = drill.volume_inventory(NAME)
        self.assertIn("pageToken=next", cloud.call_args_list[1].args[1])
        with self.assertRaisesRegex(ValueError, "Home ledger"):
            drill.validate_baseline({"pre_update_backup": NAME}, SETTINGS, {**BACKUP, "volumeCount": 2}, volumes)

    def test_prepare_owns_only_a_new_project_and_refuses_changed_terraform_inputs(self):
        (drill.ROOT / "private").mkdir(exist_ok=True)
        with tempfile.TemporaryDirectory(dir=drill.ROOT / "private") as root:
            directory = pathlib.Path(root) / "drill"
            with patch.object(drill, "call", side_effect=[BACKUP, {"volumeBackups": VOLUMES}]), patch.object(drill.subprocess, "check_output", return_value=json.dumps({"environments": {"production": {"pre_update_backup": NAME}}})):
                receipt = drill.prepare(directory, {"pre_update_backup": NAME}, SETTINGS, "aidash-production-state")
            self.assertRegex(receipt["project_id"], r"^aidash-drill-[a-f0-9]{12}$")
            self.assertEqual(drill.load(directory), receipt)
            variables = drill.read(directory / "terraform.tfvars.json")
            variables["project_id"] = "aidash-production"
            drill.write(directory / "terraform.tfvars.json", variables)
            with self.assertRaisesRegex(ValueError, "inputs changed"):
                drill.load(directory)

    def test_restore_excludes_secrets_ledger_and_every_workload_but_does_not_claim_acceptance(self):
        receipt = {"id": "012345abcdef", "project_id": "aidash-drill-012345abcdef", "settings": SETTINGS, "backup": NAME, "state_bucket": "aidash-production-state"}
        prefix = "projects/aidash-drill-012345abcdef/locations/us-central1"
        target = {"project_id": receipt["project_id"], "cluster": prefix + "/clusters/restore-drill", "restore_plan": prefix + "/restorePlans/pre-update-drill"}
        plan = {"cluster": target["cluster"], "backupPlan": NAME.rsplit("/backups/", 1)[0], "restoreConfig": {"namespacedResourceRestoreMode": "MERGE_SKIP_ON_CONFLICT", "volumeDataRestorePolicy": "RESTORE_VOLUME_DATA_FROM_BACKUP", "selectedNamespaces": {"namespaces": [SETTINGS["namespace"]]}, "clusterResourceRestoreScope": {"noGroupKinds": True}}}
        body = drill.restore_body(receipt)
        exclusions = body["filter"]["exclusionFilters"]
        self.assertEqual({e["groupKind"]["resourceKind"] for e in exclusions}, {"Secret", "PersistentVolumeClaim", "Pod", "ReplicationController", "Deployment", "StatefulSet", "ReplicaSet", "DaemonSet", "Job", "CronJob"})
        self.assertEqual(exclusions[-1]["name"], SETTINGS["ledger_claim"])
        normalized = copy.deepcopy(body["filter"])
        for selectors in normalized.values():
            selectors.reverse()
            for selector in selectors:
                if selector["groupKind"]["resourceGroup"] == "":
                    del selector["groupKind"]["resourceGroup"]
        self.assertTrue(drill.same_filter(normalized, body["filter"]))
        self.assertFalse(drill.same_filter({"exclusionFilters": []}, body["filter"]))
        result = {**body, "filter": normalized, "state": "SUCCEEDED", "volumesRestoredCount": 1, "completeTime": "2026-10-23T01:00:00Z"}
        with tempfile.TemporaryDirectory() as root, patch.object(drill, "outputs", return_value=target), patch.object(drill.subprocess, "check_output", return_value=json.dumps({"environments": {"production": {"pre_update_backup": NAME}}})), patch.object(drill, "call", side_effect=[BACKUP, {"volumeBackups": VOLUMES}, plan, {}, urllib.error.HTTPError("restore", 404, "pending", {}, None), result]) as cloud, patch.object(drill.time, "sleep"):
            directory = pathlib.Path(root)
            drill.restore(directory, receipt)
            self.assertEqual(receipt["phase"], "resources_restored")
            self.assertFalse(receipt["application_verified"])
            self.assertEqual(cloud.call_args_list[3].args[2], body)

    def test_human_acceptance_requires_every_check_and_the_two_hour_target(self):
        for seconds, accepted in [(3600, True), (7201, False)]:
            with self.subTest(seconds=seconds), tempfile.TemporaryDirectory() as root:
                receipt = {"phase": "resources_restored", "prepared_at": "2026-10-23T00:00:00Z", "restore_completed_at": "2026-10-23T00:30:00Z"}
                evidence = {"performed_by": "drill runner", "revision": "a" * 40, "image_digests": ["sha256:" + "b" * 64], "fault_at": "2026-10-23T00:00:00+00:00", "verified_at": "2026-10-23T01:00:00+00:00" if seconds == 3600 else "2026-10-23T02:00:01+00:00", "checks": dict.fromkeys(drill.CHECKS, True), "report": "private/raw-report.json"}
                drill.record(pathlib.Path(root), receipt, evidence)
                self.assertEqual(receipt["application_verified"], accepted)
                self.assertEqual(receipt["measured_rto_seconds"], seconds)
                evidence["checks"]["ledger_preserved"] = False
                drill.record(pathlib.Path(root), receipt, evidence)
                self.assertFalse(receipt["application_verified"])

    def test_restore_refuses_a_live_writer_marker_after_prepare(self):
        receipt = {"id": "012345abcdef", "project_id": "aidash-drill-012345abcdef", "settings": SETTINGS, "backup": NAME, "state_bucket": "aidash-production-state"}
        prefix = "projects/aidash-drill-012345abcdef/locations/us-central1"
        target = {"project_id": receipt["project_id"], "cluster": prefix + "/clusters/restore-drill", "restore_plan": prefix + "/restorePlans/pre-update-drill"}
        plan = {"cluster": target["cluster"], "backupPlan": NAME.rsplit("/backups/", 1)[0], "restoreConfig": {"namespacedResourceRestoreMode": "MERGE_SKIP_ON_CONFLICT", "volumeDataRestorePolicy": "RESTORE_VOLUME_DATA_FROM_BACKUP", "selectedNamespaces": {"namespaces": [SETTINGS["namespace"]]}, "clusterResourceRestoreScope": {"noGroupKinds": True}}}
        for entry in [{"pre_update_backup": NAME, "writers_started": True}, {"pre_update_backup": NAME + "0"}]:
            with self.subTest(entry=entry), tempfile.TemporaryDirectory() as root, patch.object(drill, "outputs", return_value=target), patch.object(drill.subprocess, "check_output", return_value=json.dumps({"environments": {"production": entry}})), patch.object(drill, "call", side_effect=[BACKUP, {"volumeBackups": VOLUMES}, plan, {}, {**drill.restore_body(receipt), "state": "SUCCEEDED", "volumesRestoredCount": 1}]) as cloud:
                with self.assertRaises(ValueError):
                    drill.restore(pathlib.Path(root), receipt)
                self.assertTrue(all(call.args[0] == "GET" for call in cloud.call_args_list))

    def test_release_evidence_requires_typed_revision_digests_and_receipt_milestones(self):
        valid = {"performed_by": "drill runner", "revision": "a" * 40, "image_digests": ["sha256:" + "b" * 64], "fault_at": "2026-10-23T00:00:00Z", "verified_at": "2026-10-23T01:00:00Z", "checks": dict.fromkeys(drill.CHECKS, True), "report": "private/raw-report.json"}
        invalid = [("revision", True), ("revision", "short"), ("image_digests", [""]), ("image_digests", "sha256:" + "b" * 64), ("image_digests", [True]), ("image_digests", ["sha256:fixture"]), ("report", {"path": "report"}), ("performed_by", "  "), ("fault_at", "2026-10-23T00:10:00Z"), ("verified_at", "2026-10-23T00:20:00Z")]
        for field, value in invalid:
            with self.subTest(field=field, value=value), tempfile.TemporaryDirectory() as root:
                receipt = {"phase": "resources_restored", "prepared_at": "2026-10-23T00:00:00Z", "restore_completed_at": "2026-10-23T00:30:00Z"}
                evidence = {**valid, field: value}
                with self.assertRaises(ValueError):
                    drill.record(pathlib.Path(root), receipt, evidence)
                self.assertNotIn("application_verified", receipt)
                self.assertFalse((pathlib.Path(root) / "human-evidence.json").exists())

    def test_cleanup_deletes_child_volume_restores_before_destroying_infrastructure(self):
        receipt = {"project_id": "aidash-drill-012345abcdef", "restore": "projects/aidash-drill-012345abcdef/locations/us-central1/restorePlans/pre-update-drill/restores/drill-012345abcdef"}
        calls = []
        def cloud(method, resource):
            calls.append((method, resource))
            if method == "DELETE":
                if not resource.endswith("?force=true"):
                    raise urllib.error.HTTPError(resource, 400, "VolumeRestores exist", {}, None)
                return {}
            raise urllib.error.HTTPError(resource, 404, "deleted", {}, None)
        with tempfile.TemporaryDirectory() as root, patch.object(drill, "call", side_effect=cloud), patch.object(drill, "terraform") as terraform:
            drill.destroy(pathlib.Path(root), receipt)
            self.assertEqual(receipt["phase"], "destroyed")
            self.assertEqual(calls, [("DELETE", receipt["restore"] + "?force=true"), ("GET", receipt["restore"])])
            self.assertEqual(terraform.call_count, 2)

    def test_copied_release_images_use_target_repository_and_preserve_exact_digests(self):
        receipt = {"project_id": "aidash-drill-012345abcdef", "settings": SETTINGS}
        release = {"trusted_sha": "a" * 40, "images": {"app": "us-central1-docker.pkg.dev/aidash-production/aidash/app@sha256:" + "1" * 64, "control": "us-central1-docker.pkg.dev/aidash-production/aidash/control@sha256:" + "2" * 64}}
        with tempfile.TemporaryDirectory() as root:
            directory = pathlib.Path(root)
            drill.image_references(directory, receipt, release)
            copied = drill.read(directory / "release.json")
            self.assertEqual(copied["trusted_sha"], release["trusted_sha"])
            self.assertEqual(copied["images"]["app"], "us-central1-docker.pkg.dev/aidash-drill-012345abcdef/restore-drill/app@sha256:" + "1" * 64)
            self.assertEqual(copied["images"]["control"], "us-central1-docker.pkg.dev/aidash-drill-012345abcdef/restore-drill/control@sha256:" + "2" * 64)
            self.assertEqual(drill.read(directory / "image-map.json")["app"]["source"], release["images"]["app"])
            for invalid in ["app:latest", "app@sha256:short"]:
                with self.subTest(invalid=invalid), self.assertRaisesRegex(ValueError, "SHA-256"):
                    drill.image_references(directory, receipt, {"images": {"app": invalid}})

    def test_cleanup_refuses_a_source_project_restore(self):
        receipt = {"project_id": "aidash-drill-012345abcdef", "restore": "projects/aidash-production/locations/us-central1/restorePlans/production/restores/real"}
        with patch.object(drill, "call") as cloud, patch.object(drill, "terraform") as terraform:
            with self.assertRaisesRegex(ValueError, "source-project"):
                drill.destroy(pathlib.Path("unused"), receipt)
            cloud.assert_not_called()
            terraform.assert_not_called()


if __name__ == "__main__":
    unittest.main()
