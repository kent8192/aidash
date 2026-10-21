"""Release lifecycle and authorization at the actual gh JSON/API boundary."""

from copy import deepcopy
from datetime import datetime, timedelta, timezone
import io
import json
import os
from pathlib import Path
import sys
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import patch
from urllib.parse import parse_qs, unquote, urlsplit
import zipfile

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "control"))
import releases
from policy import IMAGE_KINDS, Refused

REPOSITORY = "example/aidash"
BOT = "aidash-releases[bot]"
MAIN = "a" * 40
SOURCE = "b" * 40
NEXT = "c" * 40
CONFIG = {"repository": REPOSITORY, "develop_branch": "develop/0.1.0", "automation_branch": "develop/0.1.0", "trusted_branch": "main", "project_id": "aidash-test", "release_app_bot_login": BOT}


def workflow_run(identifier, name="CI", sha=SOURCE, event="push"):
    return {"id": identifier, "workflow_id": 10 if name == "CI" else 20,
            "name": name, "path": ".github/workflows/ci.yml" if name == "CI" else ".github/workflows/gcp-environments.yml",
            "head_sha": sha, "head_branch": CONFIG["develop_branch"], "status": "completed", "conclusion": "success", "event": event,
            "repository": {"full_name": REPOSITORY}, "head_repository": {"full_name": REPOSITORY},
            "run_attempt": 1, "created_at": "2026-10-12T01:02:03Z"}


def event(run):
    return {"action": "completed", "repository": {"full_name": REPOSITORY}, "workflow_run": deepcopy(run)}


def receipt(sha=SOURCE, trusted=None):
    return {"protocol": "aidash-staging-receipt/1", "source_sha": sha, "trusted_sha": trusted or sha, "status": "ready", "generation": 2, "run_id": 200,
            "images": {kind: f"us-central1-docker.pkg.dev/aidash-test/aidash/{kind}@sha256:{'d' * 64}" for kind in IMAGE_KINDS}}


class GitHubTransport:
    """A repository service model; authorization remains in the real CLI owner."""

    def __init__(self):
        self.branches = {"main": {"name": "main", "protected": True, "commit": {"sha": MAIN}}, CONFIG["develop_branch"]: {"name": CONFIG["develop_branch"], "protected": True, "commit": {"sha": SOURCE}}}
        self.parents = {MAIN: set(), SOURCE: {MAIN}, NEXT: {SOURCE, MAIN}}
        self.runs = {100: workflow_run(100)}
        self.prs = []
        self.refs = {}
        self.checks = []
        self.artifacts = []
        self.zipfiles = {}
        self.writes = []
        self.branch_reads = 0
        self.advance_on_read = None

    def add_staging(self, value=None, filename="staging-receipt.json", age=timedelta(minutes=5)):
        self.runs[200] = workflow_run(200, "GCP environments", MAIN, "workflow_run")
        value = receipt() if value is None else value
        content = io.BytesIO()
        with zipfile.ZipFile(content, "w") as zipped:
            zipped.writestr(filename, json.dumps(value))
        self.zipfiles[300] = content.getvalue()
        created = (datetime.now(timezone.utc) - age).strftime("%Y-%m-%dT%H:%M:%SZ")
        self.artifacts = [{"id": 300, "name": "staging-receipt", "expired": False, "size_in_bytes": len(self.zipfiles[300]), "created_at": created,
                           "workflow_run": {"id": 200, "head_sha": MAIN, "head_branch": CONFIG["develop_branch"]}}]

    def managed_pr(self, sha=SOURCE, number=1):
        metadata = {"protocol": "aidash-release/1", "source_sha": sha, "source_branch": CONFIG["develop_branch"], "ci_run_id": 100}
        return {"number": number, "node_id": f"PR_{number}", "draft": True, "state": "open", "user": {"login": BOT, "type": "Bot"},
                "head": {"ref": f"release/20261012T010203Z-{sha[:12]}", "sha": sha, "repo": {"full_name": REPOSITORY}},
                "base": {"ref": "main", "repo": {"full_name": REPOSITORY}}, "body": f"<!-- aidash-release:{json.dumps(metadata)} -->"}

    def __call__(self, *args, data=None, timeout=None):
        if args[:2] != ("gh", "api"):
            raise AssertionError("The release owner must use the gh API transport")
        if len(args) == 3 and args[2].endswith("/zip"):
            return self.zipfiles[int(args[2].split("/")[-2])]
        self.assert_api(args, data)
        method, endpoint = args[3], args[4]
        parsed = urlsplit(endpoint)
        path = parsed.path.removeprefix(f"repos/{REPOSITORY}/")
        query = parse_qs(parsed.query)
        body = json.loads(data) if data is not None else None
        if method != "GET":
            self.writes.append((method, path, deepcopy(body)))
        if endpoint == "graphql":
            for pr in self.prs:
                if pr["node_id"] == body["variables"]["id"]:
                    pr["draft"] = False
            response = {"data": {"markPullRequestReadyForReview": {"pullRequest": {"id": body["variables"]["id"]}}}}
        elif endpoint == f"repos/{REPOSITORY}":
            response = {"default_branch": CONFIG["develop_branch"]}
        elif path.startswith("branches/"):
            branch = unquote(path.removeprefix("branches/"))
            if branch == CONFIG["develop_branch"]:
                self.branch_reads += 1
                if self.advance_on_read == self.branch_reads:
                    self.branches[branch]["commit"]["sha"] = NEXT
            response = self.branches[branch]
        elif path.startswith("compare/"):
            before, after = path.removeprefix("compare/").split("...")
            response = {"merge_base_commit": {"sha": before if before == after or before in self.parents[after] else MAIN}, "status": "identical" if before == after else "ahead" if before in self.parents[after] else "diverged"}
        elif path in {"actions/workflows/ci.yml", "actions/workflows/gcp-environments.yml"}:
            ci = path.endswith("/ci.yml")
            response = {"id": 10 if ci else 20, "path": ".github/workflows/ci.yml" if ci else ".github/workflows/gcp-environments.yml", "name": "CI" if ci else "GCP environments", "state": "active"}
        elif path.endswith("/runs") and path.startswith("actions/workflows/"):
            identifier = 10 if "/ci.yml/" in path else 20
            runs = [item for item in self.runs.values() if item["workflow_id"] == identifier]
            if "head_sha" in query:
                runs = [item for item in runs if item["head_sha"] == query["head_sha"][0]]
            runs = sorted(runs, key=lambda item: item["id"], reverse=True)
            size, page = int(query.get("per_page", ["30"])[0]), int(query.get("page", ["1"])[0])
            response = {"workflow_runs": runs[(page - 1) * size:page * size]}
        elif path == "actions/artifacts":
            response = {"artifacts": [value for value in self.artifacts if value["name"] == query["name"][0]]}
        elif path.endswith("/artifacts"):
            response = {"artifacts": self.artifacts if int(path.split("/")[2]) == 200 else []}
        elif path.startswith("actions/runs/"):
            response = self.runs[int(path.split("/")[2])]
        elif path.startswith("git/matching-refs/"):
            ref = "refs/" + path.removeprefix("git/matching-refs/")
            response = [{"ref": value, "object": {"sha": sha}} for value, sha in self.refs.items() if value.startswith(ref)]
        elif path == "git/refs":
            self.refs[body["ref"]] = body["sha"]
            response = {"ref": body["ref"], "object": {"sha": body["sha"]}}
        elif path == "pulls" and method == "POST":
            number = max([item["number"] for item in self.prs] + [0]) + 1
            response = self.managed_pr(self.refs["refs/heads/" + body["head"]], number)
            response.update(body)
            response["head"] = {"ref": body["head"], "sha": self.refs["refs/heads/" + body["head"]], "repo": {"full_name": REPOSITORY}}
            response["base"] = {"ref": body["base"], "repo": {"full_name": REPOSITORY}}
            self.prs.append(response)
        elif path == "pulls":
            response = [value for value in self.prs if value["state"] == "open"]
        elif path.startswith("pulls/"):
            response = next(pr for pr in self.prs if pr["number"] == int(path.split("/")[1]))
            response.update(body)
        elif path.startswith("commits/") and path.endswith("/check-runs"):
            response = {"check_runs": [value for value in self.checks if value["head_sha"] == path.split("/")[1]]}
        elif path == "check-runs":
            response = dict(body, id=len(self.checks) + 1, app={"slug": "aidash-releases"})
            self.checks.append(response)
        elif path.startswith("check-runs/"):
            response = next(value for value in self.checks if value["id"] == int(path.split("/")[1]))
            response.update(body)
        else:
            raise AssertionError((method, path, body))
        return json.dumps(deepcopy(response)).encode()

    def assert_api(self, args, data):
        if args[2] != "--method" or (data is not None and args[5:] != ("--input", "-")):
            raise AssertionError("API bodies must be passed as JSON stdin without a shell")


class ReleaseLifecycleTests(unittest.TestCase):
    def setUp(self):
        self.github = GitHubTransport()
        self.transport = patch.object(releases, "run", self.github)
        self.transport.start()
        self.addCleanup(self.transport.stop)

    def test_cli_creates_exact_draft_and_retry_preserves_candidate_and_unrelated_pr(self):
        unrelated = self.github.managed_pr(number=9)
        unrelated["user"] = {"login": "human", "type": "User"}
        self.github.prs.append(unrelated)
        with TemporaryDirectory() as directory:
            path = Path(directory) / "event.json"
            path.write_text(json.dumps(event(self.github.runs[100])))
            environment = {"AIDASH_GCP_CONFIG": json.dumps(CONFIG), "GITHUB_REPOSITORY": REPOSITORY, "GITHUB_EVENT_NAME": "workflow_run", "GITHUB_EVENT_PATH": str(path)}
            with patch.dict(os.environ, environment), patch.object(sys, "argv", ["releases.py", "candidate"]):
                releases.main()
                releases.main()
        candidates = [value for value in self.github.prs if value["user"]["login"] == BOT]
        self.assertEqual(len(candidates), 1)
        self.assertEqual(candidates[0]["head"]["sha"], SOURCE)
        self.assertTrue(candidates[0]["draft"])
        self.assertTrue(candidates[0]["body"].endswith(releases.ATTRIBUTION))
        self.assertEqual(unrelated["state"], "open")
        self.assertEqual(len(self.github.checks), 1)
        self.assertEqual(self.github.checks[0]["status"], "in_progress")

    def test_new_candidate_closes_only_owned_older_pr_without_changing_its_ref(self):
        self.github.prs.append(self.github.managed_pr())
        self.github.refs["refs/heads/" + self.github.prs[0]["head"]["ref"]] = SOURCE
        self.github.branches[CONFIG["develop_branch"]]["commit"]["sha"] = NEXT
        self.github.runs[101] = workflow_run(101, sha=NEXT)
        releases.create_candidate(CONFIG, event(self.github.runs[101]))
        self.assertEqual([value["state"] for value in self.github.prs], ["closed", "open"])
        self.assertEqual(self.github.prs[1]["head"]["sha"], NEXT)
        self.assertEqual(self.github.refs["refs/heads/" + self.github.prs[0]["head"]["ref"]], SOURCE)

    def test_untrusted_or_stale_ci_cannot_create_release_or_close_old_pr(self):
        changes = [lambda gh: gh.runs[100].update(event="pull_request"),
                   lambda gh: gh.runs[100].update(workflow_id=99),
                   lambda gh: gh.runs[100].update(path=".github/workflows/untrusted.yml"),
                   lambda gh: gh.runs[100]["head_repository"].update(full_name="attacker/aidash"),
                   lambda gh: gh.branches[CONFIG["develop_branch"]].update(protected=False),
                   lambda gh: gh.branches[CONFIG["develop_branch"]]["commit"].update(sha=NEXT),
                   lambda gh: gh.parents[SOURCE].clear(),
                   lambda gh: gh.runs.update({101: dict(workflow_run(101), conclusion="failure")})]
        for change in changes:
            with self.subTest(change=change):
                self.github = GitHubTransport()
                change(self.github)
                with patch.object(releases, "run", self.github), self.assertRaises(Refused):
                    releases.create_candidate(CONFIG, event(self.github.runs[100]))
                self.assertEqual(self.github.writes, [])

    def test_candidate_advance_during_request_cannot_retire_previous_pr(self):
        self.github.prs.append(self.github.managed_pr(MAIN))
        self.github.advance_on_read = 2
        with self.assertRaises(Refused):
            releases.create_candidate(CONFIG, event(self.github.runs[100]))
        self.assertEqual(self.github.writes, [])
        self.assertEqual(self.github.prs[0]["state"], "open")

    def test_valid_receipt_issues_success_and_marks_only_matching_draft_ready(self):
        self.github.prs.append(self.github.managed_pr())
        self.github.add_staging()
        releases.accept_staging(CONFIG, event(self.github.runs[200]), receipt())
        self.assertFalse(self.github.prs[0]["draft"])
        self.assertEqual(self.github.checks[0]["head_sha"], SOURCE)
        self.assertEqual(self.github.checks[0]["conclusion"], "success")
        self.assertEqual(self.github.checks[0]["external_id"], f"aidash-staging/1:200:2:{SOURCE}")

    def test_staging_before_candidate_creation_is_replayed_from_original_artifact(self):
        self.github.add_staging()
        releases.create_candidate(CONFIG, event(self.github.runs[100]))
        self.assertFalse(self.github.prs[0]["draft"])
        self.assertEqual(self.github.checks[0]["conclusion"], "success")

    def test_replay_finds_receipt_buried_by_later_reconciliation_runs(self):
        self.github.add_staging(age=timedelta(hours=3))
        for identifier in range(201, 260):
            self.github.runs[identifier] = workflow_run(identifier, "GCP environments", MAIN, "schedule")
        releases.create_candidate(CONFIG, event(self.github.runs[100]))
        self.assertFalse(self.github.prs[0]["draft"])
        self.assertEqual(self.github.checks[0]["conclusion"], "success")

    def test_replay_ignores_receipts_older_than_staging_evidence_lifetime(self):
        self.github.add_staging(age=timedelta(days=8))
        releases.create_candidate(CONFIG, event(self.github.runs[100]))
        self.assertTrue(self.github.prs[0]["draft"])
        self.assertEqual(self.github.checks[0]["status"], "in_progress")

    def test_environment_run_without_staging_receipt_is_a_no_op(self):
        self.github.prs.append(self.github.managed_pr())
        self.github.runs[201] = workflow_run(201, "GCP environments", MAIN, "schedule")
        with TemporaryDirectory() as directory:
            path = Path(directory) / "event.json"
            path.write_text(json.dumps(event(self.github.runs[201])))
            environment = {"AIDASH_GCP_CONFIG": json.dumps(CONFIG), "GITHUB_REPOSITORY": REPOSITORY, "GITHUB_EVENT_NAME": "workflow_run", "GITHUB_EVENT_PATH": str(path)}
            with patch.dict(os.environ, environment), patch.object(sys, "argv", ["releases.py", "staging"]):
                releases.main()
        self.assertEqual(self.github.writes, [])
        self.assertTrue(self.github.prs[0]["draft"])

    def test_forged_or_stale_receipts_do_not_publish_success_or_ready_pr(self):
        changes = [lambda value, gh: value.update(run_id=999),
                   lambda value, gh: value.update(status="pending"),
                   lambda value, gh: value.update(url="https://secret.preview.aidash.run"),
                   lambda value, gh: value["images"].pop("app"),
                   lambda value, gh: value["images"].update(app="attacker.invalid/app:latest"),
                   lambda value, gh: value.update(generation=True),
                   lambda value, gh: gh.artifacts.clear(),
                   lambda value, gh: gh.artifacts[0].update(expired=True),
                   lambda value, gh: gh.artifacts[0]["workflow_run"].update(id=100),
                   lambda value, gh: gh.runs[200].update(path=".github/workflows/untrusted.yml"),
                   lambda value, gh: gh.runs[200].update(event="pull_request"),
                   lambda value, gh: gh.branches[CONFIG["develop_branch"]]["commit"].update(sha=NEXT),
                   lambda value, gh: gh.runs[100].update(conclusion="failure")]
        for change in changes:
            with self.subTest(change=change):
                gh = GitHubTransport()
                gh.prs.append(gh.managed_pr())
                gh.add_staging()
                value = receipt()
                change(value, gh)
                with patch.object(releases, "run", gh), self.assertRaises(Refused):
                    releases.accept_staging(CONFIG, event(gh.runs[200]), value)
                self.assertEqual(gh.writes, [])
                self.assertTrue(gh.prs[0]["draft"])

    def test_receipt_from_a_controller_other_than_the_release_source_is_refused_even_when_it_is_a_main_ancestor(self):
        self.github.prs.append(self.github.managed_pr())
        value = receipt(trusted=MAIN)
        self.github.add_staging(value)
        with self.assertRaises(Refused):
            releases.accept_staging(CONFIG, event(self.github.runs[200]), value)
        self.assertEqual(self.github.writes, [])
        self.assertTrue(self.github.prs[0]["draft"])

    def test_fabricated_valid_shape_receipt_does_not_match_original_artifact(self):
        self.github.prs.append(self.github.managed_pr())
        self.github.add_staging()
        value = receipt()
        value["images"]["app"] = value["images"]["app"].replace("d" * 64, "e" * 64)
        with self.assertRaises(Refused):
            releases.accept_staging(CONFIG, event(self.github.runs[200]), value)
        self.assertEqual(self.github.writes, [])

    def test_changed_candidate_head_and_archive_path_are_refused(self):
        for changed_head in (False, True):
            with self.subTest(changed_head=changed_head):
                gh = GitHubTransport()
                gh.prs.append(gh.managed_pr())
                gh.add_staging(filename="../../staging-receipt.json" if not changed_head else "staging-receipt.json")
                if changed_head:
                    gh.prs[0]["head"]["sha"] = NEXT
                with patch.object(releases, "run", gh), self.assertRaises(Refused):
                    releases.accept_staging(CONFIG, event(gh.runs[200]), receipt())
                self.assertEqual(gh.writes, [])


if __name__ == "__main__":
    unittest.main()
