"""Release authorization, deployment evidence and automatic source selection contracts."""

from copy import deepcopy
import io
import json
import os
from pathlib import Path
import sys
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import patch
import urllib.error
import zipfile

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "control"))
import controller
import delivery
import kube
sys.path.insert(0, str(Path(__file__).resolve().parent))
from kubefake import FakeCluster, PROJECT
from policy import IMAGE_KINDS, Refused, transition

SOURCE, MAIN, TRUSTED, BASE = (letter * 40 for letter in "abcd")
CONFIG = {
    "repository": "kent8192/aidash", "develop_branch": "develop/0.1.0",
    "automation_branch": "develop/0.1.0", "deployment_tier": "production",
    "project_id": "aidash-production", "nonproduction_project_id": "aidash-staging",
    "state_bucket": "prod-state", "nonproduction_state_bucket": "stage-state",
    "release_bucket": "prod-releases", "notification_bucket": "prod-notifications",
    "staging_release_bucket": "stage-releases", "staging_project_id": "aidash-staging",
    "production_origin": "https://aidash.run", "release_app_bot_login": "release-bot[bot]",
}
IMAGES = {kind: f"us-central1-docker.pkg.dev/aidash-staging/aidash/{kind}@sha256:{'e' * 64}" for kind in IMAGE_KINDS}


class ObjectStore:
    def __init__(self):
        self.objects = {}
        self.state = {"environments": {}}

    def read(self, key):
        if key == "lifecycle/state.json":
            return deepcopy(self.state), "1"
        value = self.objects.get(key)
        return (deepcopy(value[0]), str(value[1])) if value else (None, "0")

    def write(self, key, value, generation):
        previous = self.objects.get(key, (None, 0))
        if str(previous[1]) != str(generation):
            raise urllib.error.HTTPError("https://storage.googleapis.com", 412, "CAS", {}, None)
        self.objects[key] = (deepcopy(value), previous[1] + 1)
        return str(previous[1] + 1)

    def mutate(self, callback):
        self.state, result = callback(deepcopy(self.state))
        return result


class ProductionAuthorizationTests(unittest.TestCase):
    def setUp(self):
        self.store = ObjectStore()
        self.receipt = {
            "protocol": "aidash-staging-receipt/1", "source_sha": SOURCE,
            "trusted_sha": SOURCE, "status": "ready", "images": IMAGES,
            "generation": 1, "run_id": 123, "occurred_at": 900,
        }
        self.store.write(f"deployments/develop/{SOURCE}.json", self.receipt, "0")
        self.pr = {
            "number": 9, "body": '<!-- aidash-release:' + json.dumps({"protocol": "aidash-release/1", "source_sha": SOURCE, "source_branch": "develop/0.1.0", "ci_run_id": 100}) + ' -->',
            "merge_commit_sha": MAIN, "merged_at": "2026-10-12T00:00:00Z",
            "base": {"ref": "main"},
            "head": {"repo": {"full_name": CONFIG["repository"]}, "sha": SOURCE, "ref": "release/20261012T000000Z-" + SOURCE[:12]},
            "user": {"login": "release-bot[bot]"},
            "merged_by": {"login": "maintainer", "type": "User"},
        }
        prefix = "repos/" + CONFIG["repository"]
        self.api = {
            prefix + "/branches/main": {"protected": True, "commit": {"sha": MAIN}},
            # Commit-associated PRs use pull-request-simple and omit merged_by.
            prefix + f"/commits/{MAIN}/pulls?per_page=100": [{"number": 9, "body": self.pr["body"]}],
            prefix + "/pulls/9": self.pr,
            prefix + "/collaborators/maintainer/permission": {"permission": "write"},
            prefix + f"/git/commits/{MAIN}": {"tree": {"sha": "tree"}, "parents": [{"sha": BASE}]},
            prefix + f"/git/commits/{SOURCE}": {"tree": {"sha": "tree"}},
            prefix + f"/compare/{BASE}...{SOURCE}": {"status": "ahead"},
            # An approved main ancestor must still be refused as the staged controller.
            prefix + f"/compare/{TRUSTED}...{MAIN}": {"status": "ahead"},
            prefix + "/actions/workflows/gcp-environments.yml": {"id": 45},
            prefix + "/actions/runs/123": {"workflow_id": 45, "status": "completed", "conclusion": "success", "head_branch": "develop/0.1.0"},
        }
        self.ci = []
        self.addCleanup(patch.stopall)
        patch.object(delivery, "Store", return_value=self.store).start()
        patch.object(delivery, "github", side_effect=lambda key: deepcopy(self.api[key])).start()
        patch.object(delivery.time, "time", return_value=1000).start()

    def authorize(self, config=None):
        return delivery.authorize_production(config or CONFIG, MAIN, lambda config, sha: self.ci.append(sha))

    def test_human_merge_preserves_original_bundle_and_records_actual_main(self):
        candidate = self.authorize()
        self.assertEqual((candidate["sha"], candidate["main_sha"]), (SOURCE, MAIN))
        self.assertEqual(candidate["release"], {"source_sha": SOURCE, "trusted_sha": SOURCE, "images": IMAGES})
        self.assertEqual(self.ci, [MAIN, SOURCE])

    def test_bundle_built_from_another_commit_is_refused_even_when_it_is_a_main_ancestor(self):
        self.store.objects[f"deployments/develop/{SOURCE}.json"] = (dict(self.receipt, trusted_sha=TRUSTED), 1)
        with self.assertRaises(Refused):
            self.authorize()
        accepted = {"source_sha": SOURCE, "trusted_sha": TRUSTED, "images": IMAGES}
        with self.assertRaises(Refused):
            delivery.authorize_production(CONFIG, MAIN, lambda config, sha: None, accepted=accepted)
        accepted["trusted_sha"] = SOURCE
        self.assertEqual(delivery.authorize_production(CONFIG, MAIN, lambda config, sha: None, accepted=accepted)["release"], accepted)

    def test_accepted_bundle_continues_when_a_new_staging_index_is_not_ready(self):
        accepted = self.authorize()["release"]
        self.api["repos/kent8192/aidash/actions/runs/123"]["status"] = "in_progress"
        with self.assertRaises(Refused):
            self.authorize()
        with patch.object(delivery, "Store", side_effect=AssertionError("mutable stage index must not be read")):
            candidate = delivery.authorize_production(CONFIG, MAIN, lambda config, sha: None, accepted=accepted)
        self.assertEqual(candidate["release"], accepted)
        with self.assertRaises(Refused):
            delivery.authorize_production(CONFIG, MAIN, lambda config, sha: None, accepted=dict(accepted, images=dict(IMAGES, app="app:latest")))

    def test_isolation_mismatch_refused_before_cloud_or_git_calls(self):
        for change in [{"project_id": "aidash-staging"}, {"state_bucket": "stage-state"}, {"staging_project_id": "other-project"}, {"notification_bucket": "prod-state"}, {"production_origin": "https://evil.example"}]:
            with self.subTest(change=change), self.assertRaises(Refused):
                self.authorize(dict(CONFIG, **change))
        self.assertEqual(self.ci, [])

    def test_main_hotfix_or_changed_merge_tree_requires_new_staging_validation(self):
        key = "repos/kent8192/aidash/git/commits/" + MAIN
        for commit in [{"tree": {"sha": "changed"}, "parents": [{"sha": BASE}]}, {"tree": {"sha": "tree"}, "parents": []}]:
            self.api[key] = commit
            with self.assertRaises(Refused):
                self.authorize()

    def test_forged_release_metadata_or_bot_merge_cannot_authorize(self):
        original = deepcopy(self.pr)
        for change in [{"user": {"login": "attacker"}}, {"merged_by": {"login": "automation", "type": "Bot"}}, {"merge_commit_sha": BASE}, {"body": "ordinary PR"}]:
            self.pr.clear()
            self.pr.update(deepcopy(original) | change)
            with self.subTest(change=change), self.assertRaises(Refused):
                self.authorize()

    def test_failed_stale_or_mutable_staging_evidence_cannot_deploy(self):
        for change in [{"status": "failed"}, {"source_sha": BASE}, {"occurred_at": -1000000}, {"images": dict(IMAGES, app="app:latest")}]:
            self.store.objects[f"deployments/develop/{SOURCE}.json"] = (dict(self.receipt, **change), 1)
            with self.subTest(change=change), self.assertRaises(Refused):
                self.authorize()


class OutcomeTests(unittest.TestCase):
    def setUp(self):
        self.lifecycle, self.receipts, self.notifications = ObjectStore(), ObjectStore(), ObjectStore()
        self.config = dict(CONFIG, deployment_tier="nonproduction", release_flow_enabled=True)
        self.lifecycle.state["environments"]["develop"] = {
            "incarnation": "f" * 12, "generation": 1, "hostname_token": "f" * 32,
            "sha": SOURCE, "status": "ready", "outcome_at": 100,
            "release": {"trusted_sha": TRUSTED, "images": IMAGES},
        }
        self.addCleanup(patch.stopall)
        patch.object(delivery, "Store", side_effect=lambda bucket: self.receipts if bucket == "prod-releases" else self.notifications).start()
        patch.object(delivery, "trusted_revision", return_value=TRUSTED).start()
        patch.dict(os.environ, {"GITHUB_RUN_ID": "123"}).start()

    def test_duplicate_outcome_is_stable_and_revalidation_versions_evidence(self):
        delivery.flush_outcomes(self.config, self.lifecycle)
        self.assertEqual(len(self.notifications.objects), 1)
        self.assertTrue(self.lifecycle.state["environments"]["develop"]["outcome_published"])
        delivery.flush_outcomes(self.config, self.lifecycle)
        entry = self.lifecycle.state["environments"]["develop"]
        entry.update(generation=2, outcome_at=200, outcome_run_id="124", outcome_published=False)
        with patch.dict(os.environ, {"GITHUB_RUN_ID": "124"}):
            delivery.flush_outcomes(self.config, self.lifecycle)
        self.assertEqual(len(self.notifications.objects), 2)
        latest, _ = self.receipts.read(f"deployments/develop/{SOURCE}.json")
        self.assertEqual((latest["generation"], latest["run_id"]), (2, 124))
        self.assertEqual(len(self.receipts.objects), 3)

    def test_retained_legacy_hosts_defer_publication_without_failing_the_run(self):
        environments = self.lifecycle.state["environments"]
        # The unchanged-running path stamps outcome_at on a retained legacy host.
        environments["pr-7"] = dict(environments["develop"], kind="pr", hostname_token=None)
        del environments["develop"]["hostname_token"]
        delivery.flush_outcomes(self.config, self.lifecycle)
        self.assertEqual(self.notifications.objects, {})
        self.assertEqual(self.receipts.objects, {})
        self.assertNotIn("outcome_published", environments["develop"])
        # Its next verified deployment issues a token and publishes normally.
        environments["develop"]["hostname_token"] = "f" * 32
        delivery.flush_outcomes(self.config, self.lifecycle)
        self.assertEqual(len(self.notifications.objects), 1)

    def test_evidence_storage_failure_keeps_deployment_ready_for_outbox_retry(self):
        with patch.object(self.notifications, "write", side_effect=RuntimeError("unavailable")), self.assertRaises(RuntimeError):
            delivery.flush_outcomes(self.config, self.lifecycle)
        self.assertEqual(self.lifecycle.state["environments"]["develop"]["status"], "ready")
        self.assertFalse(self.lifecycle.state["environments"]["develop"].get("outcome_published", False))
        delivery.flush_outcomes(self.config, self.lifecycle)
        self.assertEqual(len(self.notifications.objects), 1)

    def test_later_trusted_revalidation_recovers_evidence_without_redeploy_or_duplicate_mail(self):
        with patch.object(self.notifications, "write", side_effect=RuntimeError("unavailable")), self.assertRaises(RuntimeError):
            delivery.flush_outcomes(self.config, self.lifecycle)
        with TemporaryDirectory() as directory, patch.dict(os.environ, {"GITHUB_RUN_ID": "124", "RUNNER_TEMP": directory}), patch.object(controller, "verify_source") as verify, patch.object(controller.kube, "health", return_value=SOURCE), patch.object(controller, "public_health"):
            terraform = unittest.mock.Mock()
            terraform.outputs.return_value = {"develop": {"hostname": "f" * 32 + ".preview.aidash.run"}}
            controller.validate_staging_publication(self.config, self.lifecycle, terraform)
            delivery.flush_outcomes(self.config, self.lifecycle)
            self.assertTrue(delivery.export_staging_receipt(self.config, self.lifecycle))
            artifact = json.loads((Path(directory) / "staging-receipt.json").read_text())
        self.assertEqual(artifact["run_id"], 124)
        notice = next(iter(self.notifications.objects.values()))[0]
        self.assertEqual(notice["run_id"], "123")
        self.assertEqual(len(self.notifications.objects), 1)
        verify.assert_called_once()

    def test_failed_artifact_upload_recovers_even_after_the_mail_outcome_was_acknowledged(self):
        delivery.flush_outcomes(self.config, self.lifecycle)
        self.assertEqual(self.lifecycle.state["environments"]["develop"]["outcome_published"], "ready")
        prefix = "repos/kent8192/aidash"
        replies = {prefix + "/actions/runs/123": {"workflow_id": 1, "status": "completed", "conclusion": "success", "head_branch": "develop/0.1.0"},
                   prefix + "/actions/workflows/gcp-environments.yml": {"id": 1},
                   prefix + "/actions/runs/123/artifacts?per_page=100": {"artifacts": []}}
        with TemporaryDirectory() as directory, patch.dict(os.environ, {"GITHUB_RUN_ID": "124", "RUNNER_TEMP": directory}), patch.object(delivery, "github", side_effect=lambda key: replies[key]), patch.object(controller, "verify_source"), patch.object(controller.kube, "health", return_value=SOURCE), patch.object(controller, "public_health"):
            terraform = unittest.mock.Mock()
            terraform.outputs.return_value = {"develop": {}}
            controller.validate_staging_publication(self.config, self.lifecycle, terraform)
            delivery.flush_outcomes(self.config, self.lifecycle)
            self.assertTrue(delivery.export_staging_receipt(self.config, self.lifecycle))
        self.assertEqual(len(self.notifications.objects), 1)
        receipt, _ = self.receipts.read(f"deployments/develop/{SOURCE}.json")
        self.assertEqual(receipt["run_id"], 124)

    def test_outbox_keeps_the_original_controller_sha_across_partial_publication_retry(self):
        with patch.object(self.receipts, "write", side_effect=RuntimeError("unavailable")), self.assertRaises(RuntimeError):
            delivery.flush_outcomes(self.config, self.lifecycle)
        with patch.object(delivery, "trusted_revision", return_value=BASE):
            delivery.flush_outcomes(self.config, self.lifecycle)
        self.assertEqual(len(self.notifications.objects), 1)
        notice = next(iter(self.notifications.objects.values()))[0]
        self.assertEqual(notice["trusted_sha"], TRUSTED)
        self.assertEqual(self.lifecycle.state["environments"]["develop"]["outcome_published"], "ready")

    def test_public_artifact_omits_private_hostname_and_uses_exact_schema(self):
        delivery.flush_outcomes(self.config, self.lifecycle)
        with TemporaryDirectory() as directory, patch.dict(os.environ, {"RUNNER_TEMP": directory}):
            self.assertTrue(delivery.export_staging_receipt(self.config, self.lifecycle))
            artifact = json.loads((Path(directory) / "staging-receipt.json").read_text())
        self.assertEqual(set(artifact), {"protocol", "source_sha", "trusted_sha", "generation", "status", "images", "run_id"})
        self.assertIs(type(artifact["run_id"]), int)
        self.assertNotIn("preview.aidash.run", json.dumps(artifact))

    def test_recovered_failure_publishes_ready_in_the_same_generation(self):
        entry = self.lifecycle.state["environments"]["develop"]
        entry["status"] = "failed"
        delivery.flush_outcomes(self.config, self.lifecycle)
        with patch.object(controller.time, "time", return_value=200):
            controller.update_entry(self.lifecycle, "develop", 1, status="ready")
        delivery.flush_outcomes(self.config, self.lifecycle)
        notices = [value[0]["status"] for value in self.notifications.objects.values()]
        self.assertEqual(sorted(notices), ["failed", "ready"])
        receipt, _ = self.receipts.read(f"deployments/develop/{SOURCE}.json")
        self.assertEqual(receipt["status"], "ready")

    def test_recurring_ready_outcome_does_not_collide_with_previous_ready(self):
        delivery.flush_outcomes(self.config, self.lifecycle)
        with patch.object(controller.time, "time", return_value=200):
            controller.update_entry(self.lifecycle, "develop", 1, status="failed")
        delivery.flush_outcomes(self.config, self.lifecycle)
        with patch.object(controller.time, "time", return_value=300):
            controller.update_entry(self.lifecycle, "develop", 1, status="ready")
        delivery.flush_outcomes(self.config, self.lifecycle)
        self.assertEqual(len(self.notifications.objects), 3)
        latest, _ = self.receipts.read(f"deployments/develop/{SOURCE}.json")
        self.assertEqual(latest["occurred_at"], 300)


class AutomaticStagingTests(unittest.TestCase):
    def test_failed_production_stays_fenced_when_the_staging_index_changes(self):
        previous = {"main_sha": MAIN, "desired": "running", "status": "failed", "sha": SOURCE,
                    "sequence": 100, "release": {"source_sha": SOURCE, "trusted_sha": TRUSTED, "images": IMAGES}}
        changed = {"main_sha": MAIN, "sha": SOURCE, "release": dict(previous["release"], trusted_sha=BASE)}
        event = {"inputs": {"main_sha": MAIN, "retry_failed": "false"}}
        with patch.dict(os.environ, {"GITHUB_RUN_ID": "124", "GITHUB_REF": "refs/heads/main"}), patch.object(delivery, "authorize_production", return_value=changed):
            with self.assertRaises(Refused):
                controller.requests(CONFIG, event, "workflow_dispatch", {"environments": {"production": previous}})
            previous["status"] = "waiting_for_active_work"
            request = controller.requests(CONFIG, event, "workflow_dispatch", {"environments": {"production": previous}})[0]
            self.assertEqual(request["sequence"], 100)
            self.assertEqual(request["release"], previous["release"])
            previous["status"] = "failed"
            event["inputs"]["retry_failed"] = "true"
            request = controller.requests(CONFIG, event, "workflow_dispatch", {"environments": {"production": previous}})[0]
            self.assertEqual(request["sequence"], 124)

    def test_dispatch_only_continues_explicit_pending_production_progress(self):
        prefix = "repos/kent8192/aidash"
        completed = {"id": 50, "workflow_id": 1, "event": "push", "head_branch": "main", "head_sha": MAIN, "conclusion": "success", "status": "completed", "head_repository": {"full_name": "kent8192/aidash"}}
        prior = {"id": 123, "status": "completed", "conclusion": "success", "event": "workflow_dispatch", "head_sha": MAIN, "head_branch": "main"}
        replies = {
            prefix + "/actions/workflows/ci.yml": {"id": 1},
            prefix + "/branches/main": {"protected": True, "commit": {"sha": MAIN}},
            prefix + f"/actions/workflows/gcp-production.yml/runs?event=workflow_dispatch&head_sha={MAIN}&per_page=100": {"workflow_runs": [prior]},
            prefix + "/actions/runs/50": completed,
        }
        with patch.object(delivery, "github", side_effect=lambda key: deepcopy(replies[key])), patch.object(delivery, "run") as effects, patch.object(delivery, "read_production_progress") as progress:
            for status in ["ready", "failed", None, "interrupted"]:
                progress.return_value = status
                self.assertFalse(delivery.dispatch_production(CONFIG, {"workflow_run": completed}))
            effects.assert_not_called()
            progress.return_value = "waiting_for_active_work"
            self.assertTrue(delivery.dispatch_production(CONFIG, {"workflow_run": completed}))

    def test_production_progress_has_bounded_safe_schema_and_never_extracts(self):
        prior = {"id": 123, "conclusion": "success", "head_branch": "main", "head_sha": MAIN, "event": "workflow_dispatch"}
        artifact = {"id": 456, "name": "production-progress", "expired": False, "size_in_bytes": 200, "workflow_run": {"id": 123}}
        def archive(filename, value):
            data = io.BytesIO()
            with zipfile.ZipFile(data, "w") as zipped:
                zipped.writestr(filename, json.dumps(value))
            return data.getvalue()
        value = {"protocol": "aidash-production-progress/1", "main_sha": MAIN, "status": "waiting_for_active_work"}
        with patch.object(delivery, "github", return_value={"artifacts": [artifact]}), patch.object(delivery, "run") as download:
            download.return_value = archive("production-progress.json", value)
            self.assertEqual(delivery.read_production_progress(CONFIG, prior, MAIN), "waiting_for_active_work")
            for filename, content in [("../private", value), ("production-progress.json", dict(value, hostname="private.preview.aidash.run")), ("production-progress.json", dict(value, main_sha=BASE))]:
                download.return_value = archive(filename, content)
                self.assertIsNone(delivery.read_production_progress(CONFIG, prior, MAIN))

    def test_production_writers_run_the_original_staged_source_in_owned_namespaces(self):
        cluster = FakeCluster([])
        output = {"hostname": "aidash.run", "runtime_secret": "production", "provider_credentials": None,
                  "memory_recovery": {"required": True, "history_bucket": "production-history", "archive_bucket": "production-archives",
                                      "backup_service_account": f"backup@{PROJECT}.iam.gserviceaccount.com", "prune_service_account": f"prune@{PROJECT}.iam.gserviceaccount.com"},
                  "server_service_account": f"server@{PROJECT}.iam.gserviceaccount.com",
                  "worker_service_account": f"worker@{PROJECT}.iam.gserviceaccount.com"}
        kube.prepare(cluster, "production")
        runtime = {"AIDASH_OIDC_CLIENT_ID": "production-client", "AIDASH_OIDC_CLIENT_SECRET": "production-only-secret"}
        settings = kube.materialize(cluster, "production", PROJECT, output, runtime)
        kube.deploy(cluster, "production", output, {"images": IMAGES}, SOURCE, settings)
        kube.start_writers(cluster, "production")
        self.assertEqual(kube.health(cluster, "production"), SOURCE)
        self.assertEqual(kube.namespaces("production"), ("aidash-production", "aidash-production-sandbox", "aidash-production-trusted", "aidash-production-installer"))

    def test_main_dispatch_rejects_untrusted_ci_and_skips_active_deployments(self):
        prefix = "repos/kent8192/aidash"
        completed = {"id": 50, "workflow_id": 1, "event": "push", "head_branch": "main", "head_sha": MAIN, "conclusion": "success", "status": "completed", "head_repository": {"full_name": "kent8192/aidash"}}
        replies = {
            prefix + "/actions/workflows/ci.yml": {"id": 1},
            prefix + "/branches/main": {"protected": True, "commit": {"sha": MAIN}},
            prefix + f"/actions/workflows/gcp-production.yml/runs?event=workflow_dispatch&head_sha={MAIN}&per_page=100": {"workflow_runs": []},
            prefix + "/actions/runs/50": completed,
        }
        with patch.object(delivery, "github", side_effect=lambda key: deepcopy(replies[key])), patch.object(delivery, "run") as effects:
            self.assertTrue(delivery.dispatch_production(CONFIG, {"workflow_run": completed}))
            self.assertEqual(effects.call_args.args[-1], "main_sha=" + MAIN)
            effects.reset_mock()
            with self.assertRaises(Refused):
                delivery.dispatch_production(CONFIG, {"workflow_run": dict(completed, event="pull_request")})
            replies[prefix + f"/actions/workflows/gcp-production.yml/runs?event=workflow_dispatch&head_sha={MAIN}&per_page=100"]["workflow_runs"] = [{"status": "in_progress"}]
            self.assertFalse(delivery.dispatch_production(CONFIG, {"workflow_run": completed}))
            effects.assert_not_called()

    def test_main_ci_gate_requires_push_on_the_main_branch(self):
        completed = {"head_sha": MAIN, "status": "completed", "conclusion": "success", "event": "pull_request", "head_branch": "release/candidate"}
        with patch.object(controller, "github", return_value={"workflow_runs": [completed]}):
            with self.assertRaises(Refused):
                controller.ci_success(dict(CONFIG, required_ci_branch="main"), MAIN)
            completed.update(event="push", head_branch="main")
            controller.ci_success(dict(CONFIG, required_ci_branch="main"), MAIN)

    def test_exact_develop_push_creates_staging_but_a_superseded_sha_does_not(self):
        config = {"repository": "kent8192/aidash", "develop_branch": "develop/0.1.0", "release_flow_enabled": True}
        event = {"workflow_run": {"id": 100, "workflow_id": 1, "conclusion": "success", "event": "push", "head_branch": "develop/0.1.0", "head_sha": SOURCE}}
        candidate = {"sha": SOURCE, "source_ref": "develop/0.1.0", "source_repo": "kent8192/aidash", "fork": False, "approved_sha": ""}
        with patch.dict(os.environ, {"GITHUB_RUN_ID": "123"}), patch.object(controller, "github", return_value={"id": 1}), patch.object(controller, "source", return_value=candidate):
            requests = controller.requests(config, event, "workflow_run", {})
            self.assertEqual((requests[0]["action"], requests[0]["sha"]), ("up", SOURCE))
            event["workflow_run"]["head_sha"] = BASE
            self.assertEqual(controller.requests(config, event, "workflow_run", {}), [])

    def test_hostname_survives_updates_and_resume_but_changes_after_retirement(self):
        request = {"environment": "develop", "action": "up", "sequence": 1, "sha": SOURCE, "source_ref": "develop/0.1.0", "source_repo": "kent8192/aidash"}
        state, _ = transition({}, request, "f" * 12, 100, hostname_token="f" * 32)
        state, _ = transition(state, dict(request, sequence=2, sha=MAIN), "e" * 12, 200, hostname_token="e" * 32)
        self.assertEqual(state["environments"]["develop"]["hostname_token"], "f" * 32)
        state, _ = transition(state, dict(request, sequence=3, action="destroy"), "e" * 12, 300)
        state["environments"]["develop"]["status"] = "destroyed"
        state, _ = transition(state, dict(request, sequence=4), "e" * 12, 400, hostname_token="e" * 32)
        self.assertEqual(state["environments"]["develop"]["hostname_token"], "e" * 32)

    def test_production_cannot_stop_destroy_force_or_use_spot(self):
        request = {"environment": "production", "action": "up", "sequence": 1, "sha": SOURCE, "source_ref": "main", "source_repo": "kent8192/aidash"}
        state, _ = transition({}, request, "f" * 12, 100, production=True)
        self.assertFalse(state["environments"]["production"]["spot"])
        for change in [{"action": "stop"}, {"action": "destroy"}, {"force": True}, {"mode": "spot"}]:
            with self.subTest(change=change), self.assertRaises(Refused):
                transition(state, dict(request, sequence=2, **change), "f" * 12, 200, production=True)


if __name__ == "__main__":
    unittest.main()
