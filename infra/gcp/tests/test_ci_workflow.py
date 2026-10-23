"""Execute the required CI gate against full and verified release job outcomes."""

from copy import deepcopy
import json
import os
from pathlib import Path
import subprocess
import unittest

import yaml

ROOT = Path(__file__).resolve().parents[3]


class CISuccessTests(unittest.TestCase):
    def setUp(self):
        workflow = yaml.safe_load((ROOT / ".github/workflows/ci.yml").read_text())
        success = workflow["jobs"]["success"]
        self.command = next(step["run"] for step in success["steps"] if "NEEDS_JSON" in step.get("env", {}))
        self.jobs = [name for name in success["needs"] if name != "scope"]

    def outcomes(self, mode):
        needs = {name: {"result": "success" if mode == "full" else "skipped"} for name in self.jobs}
        needs["scope"] = {"result": "success", "outputs": {"mode": mode}}
        return needs

    def gate(self, needs, develop=True):
        return subprocess.run(
            ["bash", "--noprofile", "--norc", "-e", "-o", "pipefail", "-c", self.command],
            env=dict(os.environ, NEEDS_JSON=json.dumps(needs), DEVELOP_DESKTOP_CI="true" if develop else "false"), cwd=ROOT,
            capture_output=True, text=True, timeout=10,
        )

    def test_verified_release_and_main_do_not_require_repeated_full_jobs(self):
        for mode in ("release", "main"):
            with self.subTest(mode=mode):
                result = self.gate(self.outcomes(mode))
                self.assertEqual(result.returncode, 0, result.stderr)

    def test_full_ci_requires_every_test_partition_and_supporting_job(self):
        needs = self.outcomes("full")
        self.assertEqual(self.gate(needs).returncode, 0)
        for name in self.jobs:
            for status in ("failure", "cancelled", "skipped"):
                with self.subTest(job=name, status=status):
                    changed = deepcopy(needs)
                    changed[name]["result"] = status
                    self.assertNotEqual(self.gate(changed).returncode, 0)

    def test_application_pr_keeps_full_ci_without_repeating_its_separate_desktop_workflow(self):
        needs = self.outcomes("full")
        needs["desktop"] = {"result": "skipped"}
        result = self.gate(needs, develop=False)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_lightweight_gate_requires_successful_verification_and_skipped_full_jobs(self):
        needs = self.outcomes("release")
        for scope in ({"result": "failure", "outputs": {"mode": "release"}},
                      {"result": "cancelled", "outputs": {"mode": "release"}},
                      {"result": "success", "outputs": {}},
                      {"result": "success", "outputs": {"mode": "unrecognized"}}):
            with self.subTest(scope=scope):
                self.assertNotEqual(self.gate(dict(needs, scope=scope)).returncode, 0)
        for name in self.jobs:
            with self.subTest(unexpected_full_job=name):
                changed = deepcopy(needs)
                changed[name]["result"] = "success"
                self.assertNotEqual(self.gate(changed).returncode, 0)


if __name__ == "__main__":
    unittest.main()
