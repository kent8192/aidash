"""Run the apply shell against the single-artifact download layout."""

import io
import json
import os
from pathlib import Path
import shlex
import subprocess
import sys
from tempfile import TemporaryDirectory
import unittest
import zipfile

import yaml


ROOT = Path(__file__).resolve().parents[4]


class EnvironmentArtifactTests(unittest.TestCase):
    def run_finish(self, missing=None):
        workflow = yaml.safe_load(
            (ROOT / ".github/workflows/gcp-environments.yml").read_text()
        )
        steps = workflow["jobs"]["apply"]["steps"]
        reconcile = next(step["run"] for step in steps if "HAS_BUILD" in step.get("env", {}))
        with TemporaryDirectory(prefix="aidash artifacts ") as directory:
            runner = Path(directory)
            # download-artifact v5+ extracts a single match directly into `path`,
            # including a single match selected through `pattern`.
            for step in steps:
                if not step.get("uses", "").startswith("actions/download-artifact@"):
                    continue
                settings = step["with"]
                artifact = settings.get("name", settings.get("pattern", ""))
                request = artifact.startswith("gcp-request-")
                filename = "aidash-request.json" if request else "release.json"
                if filename == missing:
                    continue
                target = Path(settings["path"].replace("${{ runner.temp }}", directory))
                archive = io.BytesIO()
                with zipfile.ZipFile(archive, "w") as package:
                    package.writestr(filename, json.dumps({"fixture": "request" if request else "release"}))
                with zipfile.ZipFile(archive) as package:
                    package.extractall(target)

            # Record the actual shell-expanded CLI arguments without starting
            # the credentialed controller. File lookup remains real below.
            calls = runner / "calls.json"
            executable = runner / "bin/python3"
            executable.parent.mkdir()
            recorder = (
                "import json, os, sys\n"
                "with open(os.environ['WORKFLOW_CALLS'], 'w') as output:\n"
                "    json.dump(sys.argv[1:], output)\n"
            )
            executable.write_text(
                f"#!/bin/sh\nexec {shlex.quote(sys.executable)} -c {shlex.quote(recorder)} \"$@\"\n"
            )
            executable.chmod(0o700)
            environment = dict(os.environ, HAS_BUILD="true", RUNNER_TEMP=directory,
                               GITHUB_RUN_ID="123", WORKFLOW_CALLS=str(calls))
            environment["PATH"] = str(executable.parent) + os.pathsep + os.environ["PATH"]
            result = subprocess.run(
                ["bash", "--noprofile", "--norc", "-e", "-o", "pipefail", "-c", reconcile],
                cwd=ROOT, env=environment, capture_output=True, text=True, timeout=10,
            )
            if missing:
                self.assertNotEqual(result.returncode, 0, "Missing inputs must stop apply")
                self.assertFalse(calls.exists(), "Do not start the controller without its inputs")
                return
            self.assertEqual(result.returncode, 0, result.stderr)
            arguments = json.loads(calls.read_text())
            self.assertEqual(arguments[1], "finish")
            for option, fixture in [("--request", "request"), ("--release", "release")]:
                path = Path(arguments[arguments.index(option) + 1])
                self.assertTrue(path.is_file(), f"Workflow input does not exist: {path}")
                self.assertEqual(json.loads(path.read_text()), {"fixture": fixture})

    def test_finish_receives_the_downloaded_request_and_release(self):
        self.run_finish()

    def test_missing_download_stops_before_the_controller(self):
        for missing in ["aidash-request.json", "release.json"]:
            with self.subTest(missing=missing):
                self.run_finish(missing)
