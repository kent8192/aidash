"""Verify that Release CI keeps exactly one develop-red Issue in step with its runs."""
import importlib.util
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("develop_red", ROOT / "scripts" / "develop_red.py")
develop_red = importlib.util.module_from_spec(spec)
spec.loader.exec_module(develop_red)
WORKFLOW = (ROOT / ".github" / "workflows" / "release-ci.yml").read_text()

SHA = "0123456789abcdef0123456789abcdef01234567"
RUN = "https://github.com/kent8192/aidash/actions/runs/1"


def plan(result, issue=None, failed=()):
    return develop_red.plan(result, issue, "develop/0.1.0", SHA, RUN, list(failed))


class PlanTests(unittest.TestCase):
    def test_first_failure_opens_a_marked_issue_naming_the_failed_jobs(self):
        [(method, path, body)] = plan("failure", failed=["Validate release source / Trunk"])

        self.assertEqual((method, path), ("POST", "issues"))
        self.assertTrue(body["body"].startswith(develop_red.MARKER))
        self.assertIn("- Validate release source / Trunk", body["body"])
        self.assertIn("`0123456789`", body["body"])
        self.assertEqual(body["labels"], ["bug", "severity:high", "priority:P1"])

    def test_later_failures_comment_on_the_open_issue(self):
        [(method, path, body)] = plan("failure", {"number": 7}, ["Federation tests"])

        self.assertEqual((method, path), ("POST", "issues/7/comments"))
        self.assertIn("still red", body["body"])

    def test_green_run_comments_then_closes_the_open_issue(self):
        actions = plan("success", {"number": 7})

        self.assertEqual([(m, p) for m, p, _ in actions], [("POST", "issues/7/comments"), ("PATCH", "issues/7")])
        self.assertEqual(actions[1][2], {"state": "closed", "state_reason": "completed"})

    def test_green_run_without_an_open_issue_writes_nothing(self):
        self.assertEqual(plan("success"), [])


class WorkflowTests(unittest.TestCase):
    def test_a_newer_push_waits_instead_of_cancelling_the_running_suite(self):
        self.assertRegex(WORKFLOW, r"(?m)^  cancel-in-progress: false$")
        self.assertNotIn("queue:", WORKFLOW)

    def test_report_runs_after_failed_but_not_cancelled_suites(self):
        job = re.search(r"(?ms)^  report:\n(.*?)(?=^  \S|\Z)", WORKFLOW).group(1)
        self.assertIn("needs: validate", job)
        self.assertIn("if: ${{ !cancelled() }}", job)
        self.assertIn("issues: write", job)
        self.assertIn("continue-on-error: true", job)
        self.assertIn("RESULT: ${{ needs.validate.result }}", job)


if __name__ == "__main__":
    unittest.main()
