"""The dispatch experiment's HTTP contract, fail-closed classification and redaction.

No live GitHub/Google request is made. Mock the HTTP opener, not the probe's
claim validation/classification, so unrelated denials cannot pass as evidence.
"""

import base64
from contextlib import redirect_stderr, redirect_stdout
import importlib.util
from io import BytesIO, StringIO
import json
from pathlib import Path
import tempfile
import time
import unittest
from unittest.mock import patch
from urllib.error import HTTPError, URLError
from urllib.parse import parse_qs, urlsplit
from urllib.response import addinfourl


SCRIPT = Path(__file__).resolve().parents[1] / "sandbox" / "wif_probe.py"
SPEC = importlib.util.spec_from_file_location("wif_probe", SCRIPT)
wif = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(wif)
SECRET = "never-record-runtime-or-returned-credential"
DENIAL = {"error": "invalid_grant", "error_description": "The given credential is rejected by the attribute condition."}


def jwt(claims):
    def encode(value):
        return base64.urlsafe_b64encode(json.dumps(value).encode()).decode().rstrip("=")
    return ".".join((encode({"alg": "RS256", "kid": "fixture"}), encode(claims), "signature"))


class Response(BytesIO):
    status = 200


class WifProbeTests(unittest.TestCase):
    def setUp(self):
        self.env = {
            "GITHUB_REPOSITORY": "kent8192/aidash", "GITHUB_EVENT_NAME": "workflow_dispatch",
            "GITHUB_REF": "refs/heads/develop/0.1.0", "GITHUB_SHA": "a" * 40,
            "GITHUB_RUN_ID": "1234", "GITHUB_RUN_ATTEMPT": "1", "WIF_REF_PROTECTED": "true",
            "GITHUB_WORKFLOW_REF": "kent8192/aidash/.github/workflows/gcp-environments.yml@refs/heads/develop/0.1.0",
            "WIF_PROVIDER": wif.PROVIDER,
            "ACTIONS_ID_TOKEN_REQUEST_URL": "https://run-actions.example.actions.githubusercontent.com/idtoken?api-version=2.0",
            "ACTIONS_ID_TOKEN_REQUEST_TOKEN": SECRET,
        }
        now = int(time.time())
        self.claims = {
            "iss": "https://token.actions.githubusercontent.com", "aud": wif.AUDIENCE,
            "repository": "kent8192/aidash", "repository_id": "1378229915", "repository_owner_id": "51869472",
            "event_name": "workflow_dispatch", "ref": self.env["GITHUB_REF"],
            "workflow_ref": self.env["GITHUB_WORKFLOW_REF"], "workflow_sha": "a" * 40,
            "run_id": "1234", "run_attempt": "1", "iat": now, "nbf": now, "exp": now + 300,
            "sub": "repo:kent8192/aidash:ref:refs/heads/develop/0.1.0",
            "unrecorded_custom_claim": SECRET,
        }

    def execute(self, oidc=None, sts=None, status=400, case=wif.CASES[0]):
        """Exercise the real entry point and artifact, with scripted HTTP responses."""
        if oidc is None:
            oidc = {"value": jwt(self.claims)}
        if sts is None:
            sts = DENIAL
        requests = []

        def open_http(opener, request, timeout):
            requests.append(request)
            value = oidc if len(requests) == 1 else sts
            if isinstance(value, Exception):
                raise value
            raw = value if isinstance(value, bytes) else json.dumps(value).encode()
            if len(requests) == 2 and status != 200:
                raise HTTPError(request.full_url, status, SECRET, {}, BytesIO(raw))
            return Response(raw)

        stdout, stderr = StringIO(), StringIO()
        with tempfile.TemporaryDirectory() as directory:
            artifact = Path(directory) / "evidence.json"
            with (patch.dict("os.environ", self.env, clear=True),
                  patch("sys.argv", [str(SCRIPT), "--case", case, "--evidence", str(artifact)]),
                  patch("urllib.request.OpenerDirector.open", autospec=True, side_effect=open_http),
                  redirect_stdout(stdout), redirect_stderr(stderr)):
                code = wif.main()
            raw_artifact = artifact.read_text()
        evidence = json.loads(raw_artifact)
        # Check all outward surfaces, including raw JWT values and reflected errors.
        outward = raw_artifact + stdout.getvalue() + stderr.getvalue()
        self.assertNotIn(SECRET, outward)
        self.assertNotIn(jwt(self.claims), outward)
        self.assertEqual(stderr.getvalue(), "")
        return code, evidence, requests

    def test_protected_ref_without_environment_forces_exact_sts_exchange(self):
        code, evidence, requests = self.execute()
        self.assertEqual((code, evidence["outcome"], evidence["reason"]),
                         (0, "expected_denial", "provider_attribute_condition"))
        self.assertTrue(evidence["claims_validated"])
        self.assertTrue(evidence["sts_attempted"])
        self.assertIsNone(evidence["environment"])
        self.assertEqual(len(requests), 2)
        oidc, sts = requests
        self.assertEqual(parse_qs(urlsplit(oidc.full_url).query)["audience"], [wif.AUDIENCE])
        self.assertEqual(oidc.get_header("Authorization"), "Bearer " + SECRET)
        self.assertEqual(sts.full_url, "https://sts.googleapis.com/v1/token")
        self.assertEqual(sts.get_method(), "POST")
        self.assertIsNone(sts.get_header("Authorization"))
        body = json.loads(sts.data)
        self.assertEqual(body, {
            "audience": "//iam.googleapis.com/projects/667820059987/locations/global/workloadIdentityPools/aidash-github/providers/github",
            "grantType": "urn:ietf:params:oauth:grant-type:token-exchange",
            "requestedTokenType": "urn:ietf:params:oauth:token-type:access_token",
            "scope": "https://www.googleapis.com/auth/cloud-platform",
            "subjectTokenType": "urn:ietf:params:oauth:token-type:jwt", "subjectToken": jwt(self.claims),
        })

    def test_unprotected_ref_with_environment_is_a_separate_experiment(self):
        self.env.update(GITHUB_REF="refs/heads/wif-negative-control", WIF_REF_PROTECTED="false")
        self.env["GITHUB_WORKFLOW_REF"] = "kent8192/aidash/.github/workflows/gcp-environments.yml@refs/heads/wif-negative-control"
        self.claims.update(ref=self.env["GITHUB_REF"], workflow_ref=self.env["GITHUB_WORKFLOW_REF"], environment="nonproduction")
        code, evidence, requests = self.execute(case=wif.CASES[1])
        self.assertEqual((code, evidence["outcome"]), (0, "expected_denial"))
        self.assertEqual(evidence["environment"], "nonproduction")
        self.assertFalse(evidence["ref_protected"])
        self.assertEqual(len(requests), 2)
        # A wrong/missing Environment does not prove the ref boundary.
        for value in (None, "production", ""):
            with self.subTest(environment=value):
                self.claims["environment"] = value
                code, evidence, requests = self.execute(case=wif.CASES[1])
                self.assertEqual((code, evidence["reason"]), (1, "wrong_environment_for_case"))
                self.assertEqual(len(requests), 1)

    def test_wrong_claims_never_reach_sts_or_count_as_denial(self):
        for key in ("iss", "aud", "repository", "repository_id", "repository_owner_id", "event_name",
                    "ref", "workflow_ref", "workflow_sha", "run_id", "run_attempt"):
            with self.subTest(claim=key):
                claims = dict(self.claims, **{key: SECRET})
                code, evidence, requests = self.execute(oidc={"value": jwt(claims)})
                self.assertEqual((code, evidence["reason"]), (1, "wrong_oidc_claims"))
                self.assertFalse(evidence["sts_attempted"])
                self.assertEqual(len(requests), 1)
        for value in ("nonproduction", "", None):
            with self.subTest(environment=value):
                code, evidence, requests = self.execute(oidc={"value": jwt(dict(self.claims, environment=value))})
                self.assertEqual((code, evidence["reason"]), (1, "wrong_environment_for_case"))
                self.assertEqual(len(requests), 1)

    def test_bad_or_expired_tokens_are_inconclusive(self):
        tokens = [SECRET, None, {}, "a.b.c", jwt(dict(self.claims, exp=0)),
                  jwt(dict(self.claims, iat=0)), jwt(dict(self.claims, nbf=int(time.time()) + 1000))]
        for token in tokens:
            with self.subTest(token_type=type(token).__name__):
                code, evidence, requests = self.execute(oidc={"value": token})
                self.assertEqual((code, evidence["outcome"]), (1, "inconclusive"))
                self.assertEqual(len(requests), 1)

    def test_wrong_context_or_endpoint_sends_no_bearer(self):
        overrides = [{"WIF_PROVIDER": "projects/123/locations/global/workloadIdentityPools/aidash-github/providers/github"},
                     {"GITHUB_EVENT_NAME": "push"}, {"WIF_REF_PROTECTED": "false"},
                     {"GITHUB_WORKFLOW_REF": "another/workflow@ref"},
                     {"ACTIONS_ID_TOKEN_REQUEST_URL": "http://run.actions.githubusercontent.com/idtoken"},
                     {"ACTIONS_ID_TOKEN_REQUEST_URL": "https://actions.githubusercontent.com.attacker.example/idtoken"},
                     {"ACTIONS_ID_TOKEN_REQUEST_TOKEN": ""}]
        for override in overrides:
            with self.subTest(override=list(override)):
                with patch.dict(self.env, override):
                    code, evidence, requests = self.execute()
                self.assertEqual((code, evidence["outcome"]), (1, "inconclusive"))
                self.assertEqual(requests, [])

    def test_success_or_any_returned_credential_is_fail_closed_and_redacted(self):
        for status, response in [(200, {"access_token": SECRET, "token_type": "Bearer"}),
                                 (200, {}), (400, dict(DENIAL, access_token=SECRET)),
                                 (403, {"id_token": SECRET})]:
            with self.subTest(status=status):
                code, evidence, requests = self.execute(sts=response, status=status)
                self.assertEqual((code, evidence["outcome"]), (1, "unexpected_allow"))
                self.assertEqual(len(requests), 2)

    def test_missing_environment_mapping_refusal_is_distinct_from_condition_denial(self):
        code, evidence, _ = self.execute(sts={"error": "invalid_grant", "error_description":
                                            "Error mapping attribute 'attribute.workflow_role': no such key: environment"})
        self.assertEqual((code, evidence["reason"]), (0, "provider_missing_environment_attribute"))

    def test_unavailable_malformed_and_unrelated_sts_failures_do_not_prove_boundary(self):
        responses = [(400, {"error": "invalid_grant", "error_description": "Token expired " + SECRET}),
                     (403, {"error": "permission_denied", "error_description": SECRET}),
                     (404, DENIAL), (503, DENIAL),
                     (400, {"error": "invalid_target", "error_description": SECRET}),
                     (400, b"not-json-" + SECRET.encode()), (400, b"x" * 65537),
                     (400, URLError(SECRET))]
        for status, response in responses:
            with self.subTest(status=status, response_type=type(response).__name__):
                code, evidence, requests = self.execute(sts=response, status=status)
                self.assertEqual((code, evidence["outcome"]), (1, "inconclusive"))
                self.assertEqual(len(requests), 2)
        code, evidence, requests = self.execute(oidc=URLError(SECRET))
        self.assertEqual((code, evidence["reason"]), (1, "transport_unavailable"))
        self.assertEqual(len(requests), 1)

    def test_redirects_are_refused_without_forwarding_the_runtime_bearer(self):
        # Real urllib redirect handling must not follow either endpoint's Location.
        requests = []

        def response(handler, request):
            requests.append(request)
            headers = {"Location": "https://attacker.example/" + SECRET}
            result = addinfourl(BytesIO(json.dumps(DENIAL).encode()), headers, request.full_url, 302)
            result.msg = "Found"
            return result

        with patch("urllib.request.HTTPSHandler.https_open", autospec=True, side_effect=response):
            evidence, code = wif.probe(wif.CASES[0], self.env)
        self.assertEqual(code, 1)
        self.assertEqual(len(requests), 1)
        self.assertEqual(evidence["oidc_http_status"], 302)
        self.assertNotIn(SECRET, json.dumps(evidence))


if __name__ == "__main__":
    unittest.main()
