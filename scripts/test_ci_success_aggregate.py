"""Verify that CI Success is cancellable yet still rejects every unsuccessful prerequisite."""
import json
from pathlib import Path
import re
import subprocess
import sys
import textwrap
import unittest


WORKFLOW = Path(__file__).resolve().parents[1] / ".github" / "workflows" / "ci.yml"
# PyYAML is not guaranteed on runners, so read the job from its fixed layout.
JOB = re.search(r"^  success:\n((?:    .*\n|\n)+)", WORKFLOW.read_text(), re.MULTILINE).group(1)
SCRIPT = textwrap.dedent(re.search(r"<<'PY'\n(.*?)\n\s*PY\n", JOB, re.DOTALL).group(1))


def run_aggregate(results):
    needs = {name: {"result": result, "outputs": {}} for name, result in results.items()}
    return subprocess.run(
        [sys.executable, "-I", "-c", SCRIPT],
        env={"NEEDS_JSON": json.dumps(needs)},
        capture_output=True,
        text=True,
    )


class CancellableAggregateTests(unittest.TestCase):
    def test_guard_lets_a_cancelled_workflow_release_its_concurrency_group(self):
        # always() stays true after cancellation and keeps a superseded run queued.
        self.assertEqual(re.findall(r"^    if: (.*)$", JOB, re.MULTILINE), ["${{ !cancelled() }}"])

    def test_every_successful_prerequisite_keeps_the_aggregate_green(self):
        completed = run_aggregate({"lint": "success", "rust-tests": "success"})

        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertIn("All required CI jobs passed.", completed.stdout)

    def test_any_unsuccessful_prerequisite_fails_the_aggregate(self):
        for result in ("failure", "cancelled", "skipped"):
            with self.subTest(result=result):
                completed = run_aggregate({"lint": "success", "rust-tests": result})

                self.assertNotEqual(completed.returncode, 0)
                self.assertIn(f"{{'rust-tests': '{result}'}}", completed.stderr)


if __name__ == "__main__":
    unittest.main()
