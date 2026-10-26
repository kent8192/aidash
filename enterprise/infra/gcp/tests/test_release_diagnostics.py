# SPDX-FileCopyrightText: 2026 kent8192
# SPDX-License-Identifier: BUSL-1.1
# See enterprise/LICENSE.

"""Public release diagnostics preserve HTTP evidence without disclosing inputs."""

from contextlib import redirect_stderr, redirect_stdout
import io
import json
import os
from pathlib import Path
import sys
from tempfile import TemporaryDirectory
import traceback
import unittest
from unittest.mock import patch

import yaml

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "control"))
import releases

WORKFLOWS = Path(__file__).resolve().parents[4] / ".github" / "workflows"


class ReleaseDiagnosticsTests(unittest.TestCase):
    def setUp(self):
        self.directory = TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.token = "ghs_private-canary-token"
        self.config = '{"password":"private-canary-password"}'
        self.private_key = "-----BEGIN PRIVATE KEY-----\nprivate-canary-key\n-----END PRIVATE KEY-----"
        executable = self.root / "gh"
        executable.write_text(
            "#!/usr/bin/env python3\n"
            "import os, pathlib, sys\n"
            "sys.stdin.buffer.read()\n"
            "data = pathlib.Path(os.environ['RESPONSE_FILE']).read_bytes()\n"
            "if '--include' not in sys.argv and data.startswith(b'HTTP/'):\n"
            "    data = data.partition(b'\\r\\n\\r\\n')[2]\n"
            "sys.stdout.buffer.write(data)\n"
            "sys.stderr.buffer.write(pathlib.Path(os.environ['STDERR_FILE']).read_bytes())\n"
            "sys.exit(int(os.environ['RESPONSE_EXIT']))\n"
        )
        executable.chmod(0o755)

    def request(self, status, body, *, exit_code=0, stderr=b"", raw=False):
        response = self.root / "response"
        if status is None:
            response.write_bytes(body)
        else:
            response.write_bytes(
                f"HTTP/2.0 {status} Response\r\nX-GitHub-Request-Id: ABCD:1234:DEAD:BEEF\r\n"
                "X-RateLimit-Remaining: 42\r\n"
                "Location: https://private.invalid/?signature=private-canary-signature\r\n\r\n".encode() + body
            )
        error_file = self.root / "stderr"
        error_file.write_bytes(stderr)
        environment = {
            "PATH": str(self.root) + os.pathsep + os.environ["PATH"],
            "GH_TOKEN": self.token, "AIDASH_GCP_CONFIG": self.config,
            "RELEASE_APP_PRIVATE_KEY": self.private_key, "GH_DEBUG": "api",
            "RESPONSE_FILE": str(response), "STDERR_FILE": str(error_file),
            "RESPONSE_EXIT": str(exit_code),
        }
        out, err = io.StringIO(), io.StringIO()
        result, failure = None, None
        endpoint = "repos/example/aidash/pulls?token=private-canary-query"
        with patch.dict(os.environ, environment), redirect_stdout(out), redirect_stderr(err):
            try:
                arguments = {"raw": True} if raw else {}
                result = releases.api(endpoint, "POST", {"body": self.config}, **arguments)
            except Exception as error:
                failure = "".join(traceback.format_exception(error))
        output = out.getvalue() + err.getvalue()
        for canary in [self.token, self.config, self.private_key, "private-canary-signature", "private-canary-query", "private-canary-response"]:
            self.assertNotIn(canary, output + (failure or ""))
        events = [json.loads(line) for line in output.splitlines() if line.startswith("{")]
        return result, failure, events

    def test_validation_failure_retains_status_request_id_and_reason_without_reflected_values(self):
        body = json.dumps({
            "message": "Validation Failed",
            "errors": [
                {"resource": "PullRequest", "code": "custom", "message": "No commits between private-canary-response and main"},
                {"resource": "private-canary-response", "field": self.token, "code": self.private_key, "message": self.config},
            ],
        }).encode()
        _, failure, events = self.request(422, body, exit_code=1, stderr=self.token.encode())
        self.assertIsNotNone(failure)
        completed = [value for value in events if value.get("event") == "github.request.failed"]
        self.assertEqual(len(completed), 1, "the failed HTTP response must survive the private subprocess boundary")
        self.assertEqual(completed[0]["http_status"], 422)
        self.assertEqual(completed[0]["request_id"], "ABCD:1234:DEAD:BEEF")
        self.assertIn("no_commits_between", completed[0]["error_categories"])
        self.assertGreaterEqual(completed[0]["elapsed_ms"], 0)

    def test_success_returns_private_response_without_logging_it(self):
        payload = {"number": 447, "body": "private-canary-response"}
        result, failure, events = self.request(201, json.dumps(payload).encode())
        self.assertIsNone(failure)
        self.assertEqual(result, payload)
        completed = [value for value in events if value.get("event") == "github.request.completed"]
        self.assertEqual(len(completed), 1)
        self.assertEqual(completed[0]["http_status"], 201)

    def test_network_failure_reports_transport_category_without_raw_stderr(self):
        _, failure, events = self.request(None, b"", exit_code=1, stderr=("connection reset by peer Authorization: Bearer " + self.token).encode())
        self.assertIsNotNone(failure)
        completed = [value for value in events if value.get("event") == "github.request.failed"]
        self.assertEqual(len(completed), 1)
        self.assertIsNone(completed[0]["http_status"])
        self.assertIn("connection_reset", completed[0]["error_categories"])

    def test_permission_failure_and_non_json_server_error_preserve_http_evidence(self):
        for status, body, category in [
            (403, json.dumps({"message": "Resource not accessible by integration", "body": self.token}).encode(), "integration_permission_denied"),
            (503, b"<html>private-canary-response</html>", "unclassified"),
            (200, json.dumps({"errors": [{"type": "FORBIDDEN", "message": self.config}]}).encode(), "unclassified"),
        ]:
            with self.subTest(status=status):
                _, failure, events = self.request(status, body, exit_code=1)
                self.assertIsNotNone(failure)
                completed = [value for value in events if value.get("event") == "github.request.failed"]
                self.assertEqual(completed[0]["http_status"], status)
                self.assertIn(category, completed[0]["error_categories"])
                if status == 200:
                    self.assertEqual(completed[0]["validation_errors"], [{"type": "FORBIDDEN"}])

    def test_malformed_success_response_is_reported_without_its_contents(self):
        _, failure, events = self.request(200, b"private-canary-response\xff")
        self.assertIsNotNone(failure)
        self.assertEqual(events[-1]["http_status"], 200)
        self.assertEqual(events[-1]["error_categories"], ["invalid_response"])

    def test_binary_artifact_is_returned_byte_for_byte_without_logging_content(self):
        artifact = b"PK\x03\x04\x00private-canary-response\xff\x00"
        result, failure, events = self.request(200, artifact, raw=True)
        self.assertIsNone(failure)
        self.assertEqual(result, artifact)
        self.assertEqual(events[-1]["http_status"], 200)

    def test_log_boundary_withholds_unknown_fields_and_mistyped_public_identifiers(self):
        output = io.StringIO()
        with redirect_stdout(output):
            releases.emit("candidate.created", pull_number=self.token, source_sha=self.config,
                          error_type=self.private_key, configuration=self.config, request_id=self.token,
                          elapsed_ms=12, token_present=True)
        for canary in [self.token, self.config, "private-canary-key"]:
            self.assertNotIn(canary, output.getvalue())
        event = json.loads(output.getvalue())
        self.assertEqual(event["elapsed_ms"], 12)
        self.assertTrue(event["token_present"])
        self.assertNotIn("configuration", event)


class ReleaseWorkflowPrivacyTests(unittest.TestCase):
    def test_controller_configuration_crosses_the_runner_boundary_as_a_secret(self):
        caller = yaml.safe_load((WORKFLOWS / "release-pr.yml").read_text())
        job = caller["jobs"]["release"]
        self.assertIn("uses", job, "public configuration must be registered as a reusable-workflow secret before any runner output")
        worker = yaml.safe_load((WORKFLOWS.parent.parent / job["uses"]).read_text())
        self.assertEqual(job["secrets"]["AIDASH_GCP_CONFIG"], "${{ vars.AIDASH_GCP_CONFIG }}")
        self.assertEqual(worker[True]["workflow_call"]["secrets"]["AIDASH_GCP_CONFIG"]["required"], True)
        for worker_job in worker["jobs"].values():
            self.assertEqual(worker_job["environment"], "release-automation")
            self.assertEqual(worker_job["env"]["AIDASH_GCP_CONFIG"], "${{ secrets.AIDASH_GCP_CONFIG }}")
            self.assertNotIn("vars.AIDASH_GCP_CONFIG", json.dumps(worker_job))


if __name__ == "__main__":
    unittest.main()
