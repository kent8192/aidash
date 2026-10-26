#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 kent8192
# SPDX-License-Identifier: BUSL-1.1
# See enterprise/LICENSE.

"""Dispatch-only negative WIF probes; credentials and raw responses stay in memory.

The runtime endpoint delivers the JWT over HTTPS. Local claim inspection checks
the experiment, not the signature: Google's forced STS exchange verifies that.
Only a recognized provider boundary denial passes. No IAM impersonation follows.
"""

import argparse
import base64
import binascii
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import re
import time
from urllib.error import HTTPError, URLError
from urllib.parse import parse_qsl, urlencode, urlsplit, urlunsplit
from urllib.request import HTTPRedirectHandler, Request, build_opener


# ADR 0081's existing nonproduction trust anchor, not a caller-selected target.
PROVIDER = "projects/667820059987/locations/global/workloadIdentityPools/aidash-github/providers/github"
AUDIENCE = "//iam.googleapis.com/" + PROVIDER
PROTECTED_REF = "refs/heads/develop/0.1.0"
REPOSITORY = "kent8192/aidash"
WORKFLOW = REPOSITORY + "/.github/workflows/gcp-environments.yml@"
STS_URL = "https://sts.googleapis.com/v1/token"
LIMIT = 65536
CASES = ("protected-ref-without-environment", "unprotected-ref-with-environment")


class ProbeFailure(Exception):
    """Only fixed, nonsecret reason codes may cross the probe boundary."""


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def request_json(request):
    """Bounded HTTPS transport; never reflect exception text or response bodies."""
    try:
        with build_opener(NoRedirect()).open(request, timeout=30) as response:
            status, body = response.status, response.read(LIMIT + 1)
    except HTTPError as error:
        with error:
            status, body = error.code, error.read(LIMIT + 1)
    except (OSError, URLError, ValueError):
        raise ProbeFailure("transport_unavailable") from None
    if len(body) > LIMIT:
        raise ProbeFailure("response_too_large")
    try:
        value = json.loads(body)
    except (ValueError, UnicodeError):
        raise ProbeFailure("malformed_response") from None
    if not isinstance(value, dict):
        raise ProbeFailure("malformed_response")
    return status, value


def oidc_request_url(raw):
    parsed = urlsplit(raw)
    host = parsed.hostname or ""
    if (parsed.scheme != "https" or not host.endswith(".actions.githubusercontent.com")
            or parsed.username or parsed.password or parsed.port not in (None, 443)
            or not parsed.path or parsed.fragment):
        raise ProbeFailure("untrusted_oidc_endpoint")
    query = [(key, value) for key, value in parse_qsl(parsed.query) if key != "audience"]
    query.append(("audience", AUDIENCE))
    return urlunsplit((parsed.scheme, parsed.netloc, parsed.path, urlencode(query), ""))


def token_claims(token):
    try:
        parts = token.split(".")
        if len(parts) != 3 or not all(re.fullmatch(r"[A-Za-z0-9_-]+", part) for part in parts):
            raise ValueError()
        header, claims = [json.loads(base64.urlsafe_b64decode(part + "=" * (-len(part) % 4)))
                          for part in parts[:2]]
        if (not isinstance(header, dict) or header.get("alg") not in ("RS256", "ES256")
                or not isinstance(header.get("kid"), str) or not header["kid"]
                or not isinstance(claims, dict)):
            raise ValueError()
        return claims
    except (AttributeError, ValueError, UnicodeError, binascii.Error):
        raise ProbeFailure("malformed_oidc_token") from None


def validate_context(case, env):
    ref, sha = env.get("GITHUB_REF", ""), env.get("GITHUB_SHA", "")
    if (env.get("GITHUB_REPOSITORY") != REPOSITORY
            or env.get("GITHUB_EVENT_NAME") != "workflow_dispatch"
            or not re.fullmatch(r"refs/heads/[A-Za-z0-9_./-]+", ref)
            or not re.fullmatch(r"[a-f0-9]{40}", sha)
            or not re.fullmatch(r"[0-9]+", env.get("GITHUB_RUN_ID", ""))
            or not re.fullmatch(r"[0-9]+", env.get("GITHUB_RUN_ATTEMPT", ""))
            or env.get("GITHUB_WORKFLOW_REF") != WORKFLOW + ref
            or env.get("WIF_PROVIDER") != PROVIDER):
        raise ProbeFailure("invalid_dispatch_context")
    protected = env.get("WIF_REF_PROTECTED")
    if case == CASES[0]:
        valid = ref == PROTECTED_REF and protected == "true"
    else:
        valid = ref != PROTECTED_REF and protected == "false"
    if not valid:
        raise ProbeFailure("wrong_ref_for_case")


def validate_claims(case, claims, env):
    required = {
        "iss": "https://token.actions.githubusercontent.com",
        "aud": AUDIENCE,
        "repository": REPOSITORY,
        "repository_id": "1378229915",
        "repository_owner_id": "51869472",
        "event_name": "workflow_dispatch",
        "ref": env["GITHUB_REF"],
        "workflow_ref": env["GITHUB_WORKFLOW_REF"],
        "workflow_sha": env["GITHUB_SHA"],
        "run_id": env["GITHUB_RUN_ID"],
        "run_attempt": env["GITHUB_RUN_ATTEMPT"],
    }
    if any(claims.get(key) != value for key, value in required.items()):
        raise ProbeFailure("wrong_oidc_claims")
    now = time.time()
    if (any(type(claims.get(key)) is not int for key in ("exp", "iat", "nbf"))
            or not now - 600 <= claims["iat"] <= now + 30
            or claims["nbf"] > now + 30 or claims["exp"] <= now
            or not isinstance(claims.get("sub"), str) or not claims["sub"]):
        raise ProbeFailure("invalid_oidc_lifetime_or_subject")
    if case == CASES[0]:
        valid = "environment" not in claims
    else:
        valid = claims.get("environment") == "nonproduction"
    if not valid:
        raise ProbeFailure("wrong_environment_for_case")


def probe(case, env):
    """Return only allowlisted evidence and an exit status. Never return a token."""
    evidence = {"schema_version": 1, "probe": "wif-negative-boundary", "case": case,
                "expected_outcome": "provider_denial",
                "started_at": datetime.now(timezone.utc).isoformat(),
                "outcome": "inconclusive", "stage": "context", "sts_attempted": False}
    try:
        validate_context(case, env)
        # Context is emitted only after its shape and fixed trust anchor are checked.
        evidence.update({"provider": PROVIDER, "repository": REPOSITORY,
                         "source_sha": env["GITHUB_SHA"], "ref": env["GITHUB_REF"],
                         "workflow_ref": env["GITHUB_WORKFLOW_REF"],
                         "ref_protected": env["WIF_REF_PROTECTED"] == "true",
                         "environment": None if case == CASES[0] else "nonproduction",
                         "run_url": f"https://github.com/{REPOSITORY}/actions/runs/{env['GITHUB_RUN_ID']}",
                         "run_attempt": int(env["GITHUB_RUN_ATTEMPT"])})
        evidence["stage"] = "github_oidc"
        bearer = env.get("ACTIONS_ID_TOKEN_REQUEST_TOKEN", "")
        if not bearer:
            raise ProbeFailure("oidc_runtime_unavailable")
        url = oidc_request_url(env.get("ACTIONS_ID_TOKEN_REQUEST_URL", ""))
        status, value = request_json(Request(url, headers={"Authorization": "Bearer " + bearer}))
        evidence["oidc_http_status"] = status
        if status != 200:
            raise ProbeFailure("oidc_request_failed")
        token = value.get("value")
        claims = token_claims(token)
        evidence["stage"] = "claims"
        validate_claims(case, claims, env)
        evidence["claims_validated"] = True
        evidence["stage"] = "google_sts"
        evidence["sts_attempted"] = True
        status, value = request_json(Request(STS_URL, data=json.dumps({
            "audience": AUDIENCE, "grantType": "urn:ietf:params:oauth:grant-type:token-exchange",
            "requestedTokenType": "urn:ietf:params:oauth:token-type:access_token",
            "scope": "https://www.googleapis.com/auth/cloud-platform",
            "subjectTokenType": "urn:ietf:params:oauth:token-type:jwt", "subjectToken": token,
        }).encode(), headers={"Content-Type": "application/json"}, method="POST"))
        evidence["sts_http_status"] = status
        # Any success or token-bearing response is an unexpected allow, including
        # a malformed response that also contains an error. Never emit its value.
        if 200 <= status < 300 or any(key in value for key in ("access_token", "accessToken", "id_token", "refresh_token")):
            evidence.update(outcome="unexpected_allow", reason="sts_issued_or_accepted_credential")
            return evidence, 1
        error, description = value.get("error"), value.get("error_description")
        if (status in (400, 403) and error in ("invalid_grant", "unauthorized_client")
                and description == "The given credential is rejected by the attribute condition."):
            evidence.update(outcome="expected_denial", reason="provider_attribute_condition")
            return evidence, 0
        # This provider's workflow_role mapping references environment. A missing
        # claim may fail there before the attribute condition is evaluated.
        if (case == CASES[0] and status == 400 and error in ("invalid_grant", "invalid_request")
                and isinstance(description, str)
                and re.fullmatch(r"Error (?:mapping attribute|evaluating the attribute condition)[^\n]*no such key: environment[.]?", description)):
            evidence.update(outcome="expected_denial", reason="provider_missing_environment_attribute")
            return evidence, 0
        raise ProbeFailure("unrecognized_sts_failure")
    except ProbeFailure as failure:
        evidence["reason"] = str(failure)
        return evidence, 1
    except Exception:
        # HTTP libraries, invalid runtime configuration, and JSON structures may
        # reflect secrets in exception text. No traceback crosses this boundary.
        evidence["reason"] = "internal_probe_failure"
        return evidence, 1
    finally:
        evidence["finished_at"] = datetime.now(timezone.utc).isoformat()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--case", choices=CASES, required=True)
    parser.add_argument("--evidence", type=Path, required=True)
    args = parser.parse_args()
    evidence, status = probe(args.case, os.environ)
    args.evidence.parent.mkdir(parents=True, exist_ok=True)
    args.evidence.write_text(json.dumps(evidence, indent=2) + "\n")
    print(json.dumps({key: evidence[key] for key in ("case", "outcome", "reason", "stage")}))
    return status


if __name__ == "__main__":
    raise SystemExit(main())
