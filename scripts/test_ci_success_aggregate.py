"""Keep CI Success cancellable so superseded runs release their concurrency group."""
from pathlib import Path
import re
import unittest


WORKFLOW = Path(__file__).resolve().parents[1] / ".github" / "workflows" / "ci.yml"
# PyYAML is not guaranteed on runners, so read the job from its fixed layout.
JOB = re.search(r"^  success:\n((?:    .*\n|\n)+)", WORKFLOW.read_text(), re.MULTILINE).group(1)
# Full and verified release outcomes are covered by infra/gcp/tests/test_ci_workflow.py.


class CancellableAggregateTests(unittest.TestCase):
    def test_guard_lets_a_cancelled_workflow_release_its_concurrency_group(self):
        # always() stays true after cancellation and keeps a superseded run queued.
        self.assertEqual(re.findall(r"^    if: (.*)$", JOB, re.MULTILINE), ["${{ !cancelled() }}"])

if __name__ == "__main__":
    unittest.main()
