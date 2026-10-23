"""Reusing full development CI must never authorize an untested release tree."""

from contextlib import redirect_stderr, redirect_stdout
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
sys.path.insert(0, str(Path(__file__).resolve().parent))
import cloud
import releases
from policy import Refused
from test_releases import BOT, CONFIG, GitHubTransport, MAIN, NEXT, REPOSITORY, SOURCE, workflow_run

MERGE = "d" * 40
TREE = "e" * 40


class ReleaseCITransport(GitHubTransport):
    def __init__(self):
        super().__init__()
        self.commits = {SOURCE: {"tree": {"sha": TREE}}, MERGE: {"tree": {"sha": TREE}, "parents": [{"sha": MAIN}, {"sha": SOURCE}]}}
        self.parents[MERGE] = {MAIN, SOURCE}
        self.prs = [self.managed_pr()]
        self.prs[0].update(merge_commit_sha=MERGE, merged_at=None, merged_by=None)

    def __call__(self, *args, **kwargs):
        # Both production GitHub adapters reach this same repository boundary.
        if len(args) == 3 and args[:2] == ("gh", "api"):
            return self("gh", "api", "--method", "GET", args[2], "--include").split(b"\r\n\r\n", 1)[1]
        path = args[4].removeprefix(f"repos/{REPOSITORY}/")
        if args[3] == "GET" and path.startswith("git/commits/"):
            value = self.commits[path.split("/")[-1]]
        elif args[3] == "GET" and path.startswith("commits/") and "/pulls" in path:
            value = [{"number": pr["number"]} for pr in self.prs]
        elif args[3] == "GET" and path.startswith("collaborators/"):
            value = {"permission": "write"}
        elif args[3] == "GET" and path.startswith("pulls/"):
            value = next(pr for pr in self.prs if pr["number"] == int(path.split("/")[1]))
        else:
            return super().__call__(*args, **kwargs)
        return b"HTTP/2.0 200 OK\r\n\r\n" + json.dumps(deepcopy(value)).encode()

    def merge(self):
        self.branches["main"]["commit"]["sha"] = MERGE
        self.prs[0].update(state="closed", merged_at="2026-10-23T00:00:00Z", merged_by={"type": "User", "login": "maintainer"})
        self.checks = [{"id": 1, "head_sha": SOURCE, "name": "Staging deployment", "app": {"slug": "aidash-releases"},
                        "status": "completed", "conclusion": "success", "external_id": f"aidash-staging/1:200:2:{SOURCE}"}]


class ReleaseCITests(unittest.TestCase):
    def setUp(self):
        self.github = ReleaseCITransport()
        for owner in (cloud, releases):
            transport = patch.object(owner, "run", side_effect=lambda *args, **kwargs: self.github(*args, **kwargs))
            transport.start()
            self.addCleanup(transport.stop)

    def verify(self, mode="release", event_name=None, sha=MERGE, event_ref="refs/heads/main"):
        event_name = event_name or ("pull_request" if mode == "release" else "push")
        value = {"repository": {"full_name": REPOSITORY}, "pull_request": deepcopy(self.github.prs[0]) if self.github.prs else {"number": 1},
                 "ref": event_ref, "after": sha}
        with TemporaryDirectory() as directory:
            path = Path(directory) / "event.json"
            path.write_text(json.dumps(value))
            environment = {"GITHUB_REPOSITORY": REPOSITORY, "GITHUB_EVENT_NAME": event_name, "GITHUB_EVENT_PATH": str(path),
                           "GITHUB_SHA": sha, "GITHUB_REF": "refs/pull/1/merge" if mode == "release" else "refs/heads/main",
                           "DEVELOP_BRANCH": CONFIG["develop_branch"], "RELEASE_APP_BOT_LOGIN": BOT, "CI_MODE": mode}
            output, error = io.StringIO(), io.StringIO()
            with patch.dict(os.environ, environment), patch.object(sys, "argv", ["releases.py", "verify-ci"]), redirect_stdout(output), redirect_stderr(error):
                try:
                    releases.main()
                except SystemExit as stopped:
                    self.assertEqual(stopped.code, 0, error.getvalue())
        return output.getvalue()

    def test_draft_release_reuses_exact_source_ci_while_staging_is_still_pending(self):
        self.verify()
        self.assertEqual(self.github.writes, [], "Read-only CI must not modify release or staging state")

    def test_release_rejects_untrusted_or_untested_candidate(self):
        changes = [
            lambda gh: gh.prs[0]["user"].update(login="attacker", type="User"),
            lambda gh: gh.prs[0]["head"]["repo"].update(full_name="attacker/aidash"),
            lambda gh: gh.prs[0].update(body=gh.prs[0]["body"].replace('"ci_run_id": 100', '"ci_run_id": 99')),
            lambda gh: gh.prs[0].update(merge_commit_sha=NEXT),
            lambda gh: gh.commits[MERGE]["tree"].update(sha="f" * 40),
            lambda gh: gh.branches[CONFIG["develop_branch"]]["commit"].update(sha=NEXT),
            lambda gh: gh.runs[100].update(conclusion="failure"),
            lambda gh: gh.runs[100].update(event="pull_request"),
            lambda gh: gh.runs.update({101: dict(workflow_run(101), status="completed", conclusion="cancelled")}),
        ]
        for change in changes:
            with self.subTest(change=change):
                self.github = ReleaseCITransport()
                change(self.github)
                with self.assertRaises(Refused):
                    self.verify()
                self.assertEqual(self.github.writes, [])

    def test_main_reuses_staged_source_even_after_develop_has_advanced(self):
        self.github.merge()
        self.github.branches[CONFIG["develop_branch"]]["commit"]["sha"] = NEXT
        self.verify("main")
        self.assertEqual(self.github.writes, [])

    def test_main_can_be_reverified_by_manual_dispatch(self):
        self.github.merge()
        self.verify("main", event_name="workflow_dispatch", event_ref="main")
        self.assertEqual(self.github.writes, [])

    def test_main_requires_human_merge_matching_tree_and_original_staging_check(self):
        changes = [
            lambda gh: gh.prs.clear(),
            lambda gh: gh.prs[0].update(merged_by={"type": "Bot", "login": BOT}),
            lambda gh: gh.prs[0]["user"].update(login="attacker", type="User"),
            lambda gh: gh.commits[MERGE]["tree"].update(sha="f" * 40),
            lambda gh: gh.checks.clear(),
            lambda gh: gh.checks[0].update(conclusion="failure"),
            lambda gh: gh.checks[0]["app"].update(slug="untrusted-app"),
            lambda gh: gh.checks[0].update(external_id=f"aidash-staging/1:200:2:{NEXT}"),
            lambda gh: gh.runs[100].update(conclusion="failure"),
        ]
        for change in changes:
            with self.subTest(change=change):
                self.github = ReleaseCITransport()
                self.github.merge()
                change(self.github)
                with self.assertRaises(Refused):
                    self.verify("main")
                self.assertEqual(self.github.writes, [])

    def test_wrong_event_cannot_claim_lightweight_release_verification(self):
        with self.assertRaises(Refused):
            self.verify(event_name="workflow_dispatch")
        self.github.merge()
        with self.assertRaises(Refused):
            self.verify("main", event_name="pull_request")


if __name__ == "__main__":
    unittest.main()
