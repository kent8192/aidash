#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 kent8192
# SPDX-License-Identifier: BUSL-1.1
# See enterprise/LICENSE.

"""Release candidates, staging gates and CI verification, executed from protected code.

PR code is never checked out or executed. Every API write uses fixed endpoints
and JSON input; the installation token remains outside deployment artifacts.
"""

import argparse
import io
from itertools import count
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import re
import time
from urllib.parse import quote, urlencode
import zipfile

from cloud import CommandError, run
from delivery import merged_release
from policy import IMAGE_KINDS, Refused
from release_diagnostics import emit, endpoint, errors, phase, response

MARKER = re.compile(r"<!-- aidash-release:(\{[^\n]*\}) -->")
RELEASE_REF = re.compile(r"release/[0-9]{8}T[0-9]{6}Z-[a-f0-9]{12}")
SHA = re.compile(r"[a-f0-9]{40}")
CHECK = "Staging deployment"
ATTRIBUTION = "🤖 Generated with [Codex](https://openai.com/codex/)"
REQUESTS = count(1)
FAILED_OUTCOMES = frozenset({"failure", "cancelled", "timed_out", "stale", "action_required"})
TERMINAL_OUTCOMES = FAILED_OUTCOMES | {"success", "neutral", "skipped"}


def api(path, method="GET", data=None, *, raw=False):
    args = ["gh", "api", "--method", method, path, "--include"]
    encoded = None
    if data is not None:
        args.extend(["--input", "-"])
        encoded = json.dumps(data).encode()
    request = {"request_number": next(REQUESTS), "method": method if method in {"GET", "POST", "PATCH", "DELETE"} else "other", "endpoint": endpoint(path)}
    started = time.monotonic()
    emit("github.request.started", **request)
    try:
        output = run(*args, data=encoded, timeout=60, env={"GH_DEBUG": "", "GH_FORCE_TTY": ""})
    except CommandError as error:
        evidence, body = response(error.stdout)
        emit("github.request.failed", **request, **evidence, **errors(body, error.stderr), exit_code=error.returncode,
             elapsed_ms=round((time.monotonic() - started) * 1000))
        raise RuntimeError("GitHub API request failed; consult the safe release diagnostics") from None
    except BaseException as error:
        emit("github.request.failed", **request, http_status=None, error_type=type(error).__name__,
             elapsed_ms=round((time.monotonic() - started) * 1000))
        raise
    evidence, body = response(output)
    try:
        if evidence["http_status"] is None:
            raise ValueError("Missing HTTP response headers")
        value = body if raw else json.loads(body) if body.strip() else None
    except (ValueError, UnicodeError):
        emit("github.request.failed", **request, **evidence, error_categories=["invalid_response"],
             elapsed_ms=round((time.monotonic() - started) * 1000))
        raise RuntimeError("GitHub returned an invalid response; private output was withheld") from None
    emit("github.request.completed", **request, **evidence, elapsed_ms=round((time.monotonic() - started) * 1000))
    return value


def pages(path, field=None):
    result = []
    for page in range(1, 101):
        separator = "&" if "?" in path else "?"
        value = api(f"{path}{separator}per_page=100&page={page}")
        items = value[field] if field else value
        result.extend(items)
        if len(items) < 100:
            return result
    raise Refused("Release inventory exceeds the bounded pagination limit")


@phase
def configuration():
    config = json.loads(os.environ["AIDASH_GCP_CONFIG"])
    repository = config.get("repository", "")
    branch = config.get("develop_branch", "")
    bot = os.environ.get("RELEASE_APP_BOT_LOGIN") or config.get("release_app_bot_login", "")
    if (
        not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repository)
        or repository != os.environ["GITHUB_REPOSITORY"]
        or not re.fullmatch(r"develop/[0-9]+\.[0-9]+\.[0-9]+", branch)
        or not re.fullmatch(r"[A-Za-z0-9_-]+\[bot\]", bot)
        or config.get("automation_branch", branch) != branch
        or config.get("trusted_branch", "main") != "main"
        or (config.get("release_app_bot_login") and config["release_app_bot_login"] != bot)
    ):
        raise Refused("Configure the repository, protected development/main branches and release App bot")
    return dict(config, release_app_bot_login=bot)


def workflow(config, filename, name):
    value = api(f"repos/{config['repository']}/actions/workflows/{filename}")
    if value.get("path") != f".github/workflows/{filename}" or value.get("name") != name or value.get("state") != "active":
        raise Refused("Unexpected trusted workflow identity")
    return value


@phase
def trusted_run(config, announced, filename, name, event_name):
    value = api(f"repos/{config['repository']}/actions/runs/{int(announced['id'])}")
    owner = workflow(config, filename, name)
    if (
        value.get("id") != announced["id"]
        or value.get("head_sha") != announced.get("head_sha")
        or value.get("workflow_id") != owner["id"]
        or value.get("path") != owner["path"]
        or value.get("name") != name
        or value.get("event") not in ({event_name} if isinstance(event_name, str) else event_name)
        or value.get("repository", {}).get("full_name") != config["repository"]
        or value.get("head_repository", {}).get("full_name") != config["repository"]
        or not SHA.fullmatch(value.get("head_sha", ""))
    ):
        raise Refused("Only the exact trusted workflow run can authorize this release")
    return value


@phase
def completed_run(config, event, filename, name, event_name, *, conclusions=frozenset({"success"})):
    if event.get("action") != "completed" or event.get("repository", {}).get("full_name") != config["repository"]:
        raise Refused("Release automation requires a same-repository completed workflow event")
    value = trusted_run(config, event["workflow_run"], filename, name, event_name)
    if value.get("status") != "completed" or value.get("conclusion") not in conclusions:
        raise Refused("The trusted workflow has not reached the required terminal outcome")
    return value


def protected_branch(config, name):
    value = api(f"repos/{config['repository']}/branches/{quote(name, safe='')}")
    if value.get("name") != name or value.get("protected") is not True or not SHA.fullmatch(value.get("commit", {}).get("sha", "")):
        raise Refused("Release automation requires protected branches")
    return value["commit"]["sha"]


def ancestor(config, before, after):
    value = api(f"repos/{config['repository']}/compare/{before}...{after}")
    if value.get("merge_base_commit", {}).get("sha") != before or value.get("status") not in {"ahead", "identical"}:
        raise Refused("Release candidates must contain the current main and superseded candidate history")


@phase
def current_candidate(config, sha):
    if not SHA.fullmatch(sha) or protected_branch(config, config["develop_branch"]) != sha:
        raise Refused("The candidate was superseded by the protected development branch")
    ancestor(config, protected_branch(config, "main"), sha)


@phase
def source_ci(config, sha):
    owner = workflow(config, "release-ci.yml", "Release CI")
    query = urlencode({"head_sha": sha, "event": "push", "branch": config["develop_branch"]})
    runs = pages(f"repos/{config['repository']}/actions/workflows/release-ci.yml/runs?{query}", "workflow_runs")
    matching = [value for value in runs if (
        value.get("workflow_id") == owner["id"]
        and value.get("path") == owner["path"]
        and value.get("head_sha") == sha
        and value.get("event") == "push"
        and value.get("head_branch") == config["develop_branch"]
        and value.get("repository", {}).get("full_name") == config["repository"]
        and value.get("head_repository", {}).get("full_name") == config["repository"]
    )]
    if not matching:
        raise Refused("No trusted CI run exists for the exact candidate")
    latest = max(matching, key=lambda item: (int(item["id"]), int(item.get("run_attempt", 1))))
    if latest.get("status") != "completed" or latest.get("conclusion") != "success":
        raise Refused("The latest exact candidate CI must succeed")
    return latest


def provenance(config, pr):
    if pr.get("user", {}).get("login") != config["release_app_bot_login"] or pr.get("user", {}).get("type") != "Bot":
        return None
    branch = pr.get("head", {}).get("ref", "")
    if not RELEASE_REF.fullmatch(branch):
        return None
    markers = MARKER.findall(pr.get("body") or "")
    if len(markers) != 1:
        raise Refused("Managed release PR provenance was altered")
    value = json.loads(markers[0])
    if (
        set(value) != {"protocol", "source_sha", "source_branch", "ci_run_id"}
        or value["protocol"] != "aidash-release/1"
        or not SHA.fullmatch(value["source_sha"])
        or value["source_branch"] != config["develop_branch"]
        or type(value["ci_run_id"]) is not int or value["ci_run_id"] < 1
        or pr.get("head", {}).get("sha") != value["source_sha"]
        or not branch.endswith("-" + value["source_sha"][:12])
        or pr.get("head", {}).get("repo", {}).get("full_name") != config["repository"]
        or pr.get("base", {}).get("ref") != "main"
        or pr.get("base", {}).get("repo", {}).get("full_name") != config["repository"]
    ):
        raise Refused("Managed release PR must retain its immutable source and repository")
    return value


def managed_prs(config):
    result = []
    for value in pages(f"repos/{config['repository']}/pulls?state=open&base=main"):
        metadata = provenance(config, value)
        if metadata is not None:
            result.append((value, metadata))
    return result


def finish_pending(config, sha, conclusion, title, summary, *, run_id=None):
    slug = config["release_app_bot_login"][:-5]
    for value in pages(f"repos/{config['repository']}/commits/{sha}/check-runs?filter=all", "check_runs"):
        if value.get("name") != CHECK or value.get("app", {}).get("slug") != slug or value.get("status") == "completed":
            continue
        data = {"status": "completed", "conclusion": conclusion, "completed_at": datetime.now(timezone.utc).isoformat(),
                "output": {"title": title, "summary": summary}}
        if run_id is not None:
            data["details_url"] = f"https://github.com/{config['repository']}/actions/runs/{run_id}"
        api(f"repos/{config['repository']}/check-runs/{value['id']}", "PATCH", data)


@phase
def retire_superseded(config):
    current = protected_branch(config, config["develop_branch"])
    for pr, metadata in managed_prs(config):
        source = metadata["source_sha"]
        if source == current:
            continue
        ancestor(config, source, current)
        finish_pending(config, source, "cancelled", "This release candidate was superseded",
                       f"Source `{source}` was superseded by protected development source `{current}`. A new release requires its own CI and staging evidence.")
        api(f"repos/{config['repository']}/pulls/{pr['number']}", "PATCH", {"state": "closed"})
        emit("candidate.superseded", pull_number=pr["number"], source_sha=source)
    return current


@phase
def check(config, sha, receipt=None):
    slug = config["release_app_bot_login"][:-5]
    existing = [value for value in pages(f"repos/{config['repository']}/commits/{sha}/check-runs", "check_runs")
                if value.get("name") == CHECK and value.get("app", {}).get("slug") == slug]
    if receipt is None and any(value.get("conclusion") == "success" for value in existing):
        emit("staging.check.preserved", source_sha=sha)
        return
    data = {"name": CHECK, "head_sha": sha, "status": "in_progress", "external_id": f"aidash-staging/1:{sha}"}
    data["output"] = {"title": "Waiting for staging evidence", "summary": f"Waiting for a trusted deployment of source `{sha}`. This includes runner queue time; readiness requires the exact staging receipt."}
    if receipt is not None:
        data.update(status="completed", conclusion="success", external_id=f"aidash-staging/1:{receipt['run_id']}:{receipt['generation']}:{receipt['trusted_sha']}",
                    output={"title": "The exact release candidate is ready in staging", "summary": f"Source: `{sha}`\nGeneration: {receipt['generation']}\nTrusted controller: `{receipt['trusted_sha']}`"})
    if existing:
        data.pop("head_sha")
        api(f"repos/{config['repository']}/check-runs/{max(existing, key=lambda item: item['id'])['id']}", "PATCH", data)
    else:
        api(f"repos/{config['repository']}/check-runs", "POST", data)


@phase
def create_candidate(config, event):
    completed = completed_run(config, event, "release-ci.yml", "Release CI", "push")
    if completed["head_branch"] != config["develop_branch"]:
        emit("candidate.ignored", reason="different_development_branch")
        return False
    sha = completed["head_sha"]
    emit("candidate.validating", source_sha=sha, ci_run_id=completed["id"], ci_attempt=completed.get("run_attempt", 1))
    current_candidate(config, sha)
    latest = source_ci(config, sha)
    if latest["id"] != completed["id"]:
        raise Refused("A newer CI attempt superseded the triggering run")
    opened = managed_prs(config)
    same = [(pr, meta) for pr, meta in opened if meta["source_sha"] == sha]
    if len(same) > 1:
        raise Refused("Multiple managed PRs exist for the same candidate")
    for _, metadata in opened:
        ancestor(config, metadata["source_sha"], sha)
    if same:
        candidate = same[0][0]
        emit("candidate.reused", pull_number=candidate["number"], source_sha=sha)
    else:
        stamp = datetime.fromisoformat(completed["created_at"].replace("Z", "+00:00")).astimezone(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
        branch = f"release/{stamp}-{sha[:12]}"
        refs = api(f"repos/{config['repository']}/git/matching-refs/heads/{branch}")
        exact = [ref for ref in refs if ref["ref"] == "refs/heads/" + branch]
        if exact and (len(exact) != 1 or exact[0]["object"]["sha"] != sha):
            raise Refused("The immutable candidate branch was changed")
        current_candidate(config, sha)
        if not exact:
            api(f"repos/{config['repository']}/git/refs", "POST", {"ref": "refs/heads/" + branch, "sha": sha})
        metadata = {"protocol": "aidash-release/1", "source_sha": sha, "source_branch": config["develop_branch"], "ci_run_id": completed["id"]}
        body = (f"Promote the validated development snapshot `{sha}` to production.\n\n"
                "This PR remains a draft until staging confirms this exact snapshot. Require CI Success and Staging deployment before a human merges it.\n\n"
                f"<!-- aidash-release:{json.dumps(metadata, separators=(',', ':'))} -->\n\n{ATTRIBUTION}")
        candidate = api(f"repos/{config['repository']}/pulls", "POST", {"head": branch, "base": "main", "title": f"Release {stamp} ({sha[:12]})", "body": body, "draft": True})
        provenance(config, candidate)
        emit("candidate.created", pull_number=candidate["number"], source_sha=sha)
    current_candidate(config, sha)
    for old, _metadata in opened:
        if old["number"] != candidate["number"]:
            api(f"repos/{config['repository']}/pulls/{old['number']}", "PATCH", {"state": "closed"})
            emit("candidate.superseded", pull_number=old["number"])
    check(config, sha)
    replay_staging(config, sha)
    return True


def download_artifact(config, completed, name, optional=False):
    artifacts = pages(f"repos/{config['repository']}/actions/runs/{completed['id']}/artifacts", "artifacts")
    owned = [value for value in artifacts if value.get("name") == name]
    if optional and not owned:
        # Scheduled reconciliations and test/PR operations publish no staging evidence.
        emit("staging.receipt.absent", run_id=completed["id"])
        return None
    if len(owned) != 1 or owned[0].get("expired") is not False or owned[0].get("workflow_run", {}).get("id") != completed["id"] or not 0 < owned[0].get("size_in_bytes", 0) <= 32768:
        raise Refused("The trusted workflow has no unique bounded staging artifact")
    archive = api(f"repos/{config['repository']}/actions/artifacts/{int(owned[0]['id'])}/zip", raw=True)
    if len(archive) > 32768:
        raise Refused("Staging artifact exceeded its bounded size")
    with zipfile.ZipFile(io.BytesIO(archive)) as zipped:
        files = zipped.infolist()
        if len(files) != 1 or files[0].filename != name + ".json" or not 0 < files[0].file_size <= 16384 or files[0].flag_bits & 1:
            raise Refused("Only the sanitized staging receipt can be consumed")
        return json.loads(zipped.read(files[0]))


@phase
def download_receipt(config, event, optional=False):
    completed = completed_run(config, event, "gcp-environments.yml", "GCP environments", {"workflow_run", "schedule", "workflow_dispatch", "issue_comment"})
    return download_artifact(config, completed, "staging-receipt", optional)


@phase
def record_attempt(config, event):
    completed = completed_run(config, event, "release-ci.yml", "Release CI", "push")
    if completed["head_branch"] != config["develop_branch"]:
        raise Refused("Only protected development Release CI can identify a staging attempt")
    identifiers = [os.environ.get(name, "") for name in ("GITHUB_RUN_ID", "GITHUB_RUN_ATTEMPT")]
    if any(not re.fullmatch(r"[1-9][0-9]{0,19}", value) for value in identifiers):
        raise Refused("A staging attempt requires its trusted run and attempt identity")
    value = {"protocol": "aidash-staging-attempt/1", "source_sha": completed["head_sha"], "ci_run_id": completed["id"],
             "run_id": int(identifiers[0]), "run_attempt": int(identifiers[1])}
    (Path(os.environ["RUNNER_TEMP"]) / "staging-attempt.json").write_text(json.dumps(value))


@phase
def fail_staging(config, completed):
    # A workflow's head_sha identifies its controller, not its deployment target.
    # Only the bounded artifact written by trusted intake identifies the source.
    if completed["event"] != "workflow_run":
        return
    attempt = download_artifact(config, completed, "staging-attempt", optional=True)
    if attempt is None:
        return
    if (not isinstance(attempt, dict)
            or set(attempt) != {"protocol", "source_sha", "ci_run_id", "run_id", "run_attempt"}
            or attempt["protocol"] != "aidash-staging-attempt/1"
            or not isinstance(attempt["source_sha"], str) or not SHA.fullmatch(attempt["source_sha"])
            or any(type(attempt[key]) is not int or attempt[key] < 1 for key in ("ci_run_id", "run_id", "run_attempt"))
            or attempt["run_id"] != completed["id"] or attempt["run_attempt"] > completed.get("run_attempt", 1)):
        raise Refused("Staging failure requires the exact trusted attempt artifact")
    matches = [meta for _, meta in managed_prs(config) if meta["source_sha"] == attempt["source_sha"] and meta["ci_run_id"] == attempt["ci_run_id"]]
    if len(matches) != 1:
        return
    source = attempt["source_sha"]
    current_candidate(config, source)
    if source_ci(config, source)["id"] != attempt["ci_run_id"]:
        raise Refused("Staging failure does not identify the original exact source CI")
    conclusion = "cancelled" if completed["conclusion"] == "cancelled" else "failure"
    finish_pending(config, source, conclusion, "Staging did not complete successfully",
                   f"The trusted staging attempt for `{source}` ended with `{completed['conclusion']}`. Inspect the linked run before retrying; no staging readiness was published.",
                   run_id=completed["id"])


@phase
def replay_staging_failure(config, sha):
    oldest = time.time() - config.get("staging_max_age_seconds", 604800)
    artifacts = pages(f"repos/{config['repository']}/actions/artifacts?{urlencode({'name': 'staging-attempt'})}", "artifacts")
    for artifact in sorted(artifacts, key=lambda value: str(value.get("created_at", "")), reverse=True):
        try:
            if datetime.fromisoformat(artifact["created_at"].replace("Z", "+00:00")).timestamp() < oldest:
                return
            if artifact.get("expired") is not False or artifact.get("workflow_run", {}).get("head_branch") != config["develop_branch"]:
                continue
            run = trusted_run(config, artifact["workflow_run"], "gcp-environments.yml", "GCP environments", "workflow_run")
            if run["head_branch"] != config["develop_branch"]:
                continue
            attempt = download_artifact(config, run, "staging-attempt")
            if (not isinstance(attempt, dict) or attempt.get("source_sha") != sha
                    or attempt.get("run_id") != run["id"] or type(attempt.get("run_attempt")) is not int
                    or not 1 <= attempt["run_attempt"] <= run.get("run_attempt", 1)):
                continue
            # A newer queued, running or deferred successful attempt must not
            # inherit the failure of an older attempt for this same source.
            if run["status"] == "completed" and run["conclusion"] in FAILED_OUTCOMES:
                fail_staging(config, run)
            return
        except (Refused, ValueError, KeyError, TypeError, AttributeError, zipfile.BadZipFile) as error:
            emit("staging.receipt.rejected", error_type=type(error).__name__)


@phase
def reconcile(config, event):
    name = os.environ.get("GITHUB_EVENT_NAME")
    if event.get("repository", {}).get("full_name") != config["repository"]:
        raise Refused("Release reconciliation requires the configured repository")
    completed = None
    if name in {"push", "workflow_dispatch"}:
        ref = "refs/heads/" + config["develop_branch"]
        if os.environ.get("GITHUB_REF") != ref or name == "push" and (event.get("ref") != ref or not SHA.fullmatch(event.get("after", ""))):
            raise Refused("Release reconciliation must run on protected development")
    elif name == "workflow_run":
        workflow_name = event.get("workflow_run", {}).get("name")
        if workflow_name == "Release CI":
            completed = completed_run(config, event, "release-ci.yml", workflow_name, "push", conclusions=TERMINAL_OUTCOMES)
        elif workflow_name == "GCP environments":
            completed = completed_run(config, event, "gcp-environments.yml", workflow_name, {"workflow_run", "schedule", "workflow_dispatch", "issue_comment"}, conclusions=TERMINAL_OUTCOMES)
        else:
            raise Refused("Unexpected release reconciliation workflow")
        if completed["head_branch"] != config["develop_branch"]:
            raise Refused("Release reconciliation requires the protected development execution branch")
    else:
        raise Refused("Unexpected release reconciliation event")
    current = retire_superseded(config)
    if completed is None:
        if any(meta["source_sha"] == current for _, meta in managed_prs(config)):
            replay_staging(config, current)
        return
    if completed["name"] == "Release CI":
        if completed["conclusion"] == "success" and completed["head_sha"] == current:
            create_candidate(config, event)
    elif completed["conclusion"] == "success":
        receipt = download_receipt(config, event, optional=True)
        if receipt is not None and receipt.get("source_sha") == current:
            accept_staging(config, event, receipt)
    else:
        replay_staging_failure(config, current)


@phase
def replay_staging(config, sha):
    # The deployment and release-PR workflows may finish in either order, and
    # five-minute reconciliations soon bury the producing run in run listings.
    # Walk only staging receipts, newest first, until they outlive the staging
    # evidence lifetime. Reuse only receipts from this trusted workflow; never
    # extract downloaded files.
    oldest = time.time() - config.get("staging_max_age_seconds", 604800)
    branch = config.get("automation_branch", config["develop_branch"])
    artifacts = pages(f"repos/{config['repository']}/actions/artifacts?{urlencode({'name': 'staging-receipt'})}", "artifacts")
    for artifact in sorted(artifacts, key=lambda value: str(value.get("created_at", "")), reverse=True):
        try:
            if datetime.fromisoformat(artifact["created_at"].replace("Z", "+00:00")).timestamp() < oldest:
                emit("staging.replay.waiting", reason="evidence_expired")
                break
            produced = artifact["workflow_run"]
            if artifact.get("expired") is not False or produced.get("head_branch") != branch:
                continue
            event = {"action": "completed", "repository": {"full_name": config["repository"]},
                     "workflow_run": {"id": produced["id"], "head_sha": produced.get("head_sha")}}
            receipt = download_receipt(config, event)
            if receipt.get("source_sha") == sha:
                accept_staging(config, event, receipt)
                emit("staging.replay.accepted", source_sha=sha)
                return
        except (Refused, ValueError, KeyError, TypeError, AttributeError, zipfile.BadZipFile) as error:
            # No current trusted acknowledgement means the PR remains a draft.
            emit("staging.receipt.rejected", error_type=type(error).__name__)
            continue
    emit("staging.replay.waiting", reason="no_matching_receipt")
    replay_staging_failure(config, sha)


@phase
def validate_receipt(config, event, receipt):
    completed = completed_run(config, event, "gcp-environments.yml", "GCP environments", {"workflow_run", "schedule", "workflow_dispatch", "issue_comment"})
    repository = api(f"repos/{config['repository']}")
    default = repository["default_branch"]
    if default != config.get("automation_branch", config["develop_branch"]) or completed["head_branch"] != default:
        raise Refused("Staging receipts must come from the protected default-branch workflow")
    protected_branch(config, default)
    required = {"protocol", "source_sha", "trusted_sha", "generation", "status", "images", "run_id"}
    if (
        not isinstance(receipt, dict) or set(receipt) != required
        or receipt["protocol"] != "aidash-staging-receipt/1" or receipt["status"] != "ready"
        or not SHA.fullmatch(receipt["source_sha"]) or not SHA.fullmatch(receipt["trusted_sha"])
        or type(receipt["run_id"]) is not int or receipt["run_id"] != completed["id"]
        or type(receipt["generation"]) is not int or receipt["generation"] < 1
        or not isinstance(receipt["images"], dict) or set(receipt["images"]) != set(IMAGE_KINDS)
    ):
        raise Refused("The sanitized staging receipt is incomplete or has the wrong run provenance")
    for kind, value in receipt["images"].items():
        expected = rf"us-central1-docker\.pkg\.dev/{re.escape(config['project_id'])}/aidash/{kind}@sha256:[a-f0-9]{{64}}"
        if not isinstance(value, str) or not re.fullmatch(expected, value):
            raise Refused("Staging receipt images must be complete immutable project-scoped digests")
    artifacts = pages(f"repos/{config['repository']}/actions/runs/{completed['id']}/artifacts", "artifacts")
    owned = [value for value in artifacts if value.get("name") == "staging-receipt"]
    if len(owned) != 1 or owned[0].get("expired") is not False or owned[0].get("workflow_run", {}).get("id") != completed["id"]:
        raise Refused("No unique unexpired staging receipt belongs to the trusted run")
    if receipt["trusted_sha"] != receipt["source_sha"]:
        raise Refused("The staged controller and images must be built from the release source")
    current_candidate(config, receipt["source_sha"])
    source_ci(config, receipt["source_sha"])
    return receipt


@phase
def accept_staging(config, event, receipt):
    validated = validate_receipt(config, event, receipt)
    if validated != download_receipt(config, event):
        raise Refused("Receipt input does not match the original trusted artifact")
    matches = [(pr, meta) for pr, meta in managed_prs(config) if meta["source_sha"] == validated["source_sha"]]
    if len(matches) != 1:
        raise Refused("The receipt must match one current immutable managed release PR")
    pr, metadata = matches[0]
    original = api(f"repos/{config['repository']}/actions/runs/{metadata['ci_run_id']}")
    owner = workflow(config, "release-ci.yml", "Release CI")
    if (original.get("head_sha") != validated["source_sha"] or original.get("head_branch") != config["develop_branch"] or original.get("event") != "push" or original.get("conclusion") != "success"
        or original.get("status") != "completed" or original.get("workflow_id") != owner["id"] or original.get("path") != owner["path"]
        or original.get("repository", {}).get("full_name") != config["repository"] or original.get("head_repository", {}).get("full_name") != config["repository"]):
        raise Refused("Release PR's original exact push CI no longer validates")
    current_candidate(config, validated["source_sha"])
    check(config, validated["source_sha"], validated)
    if pr["draft"]:
        result = api("graphql", "POST", {"query": "mutation($id:ID!){markPullRequestReadyForReview(input:{pullRequestId:$id}){pullRequest{id}}}", "variables": {"id": pr["node_id"]}})
        if result.get("errors"):
            raise Refused("GitHub refused to mark the validated release PR ready")
        emit("candidate.ready", pull_number=pr["number"], source_sha=validated["source_sha"])
    else:
        emit("candidate.already_ready", pull_number=pr["number"])
    return True


@phase
def verify_ci(config, event, sha, mode):
    """Reuse exact development CI only for an unchanged, authenticated release."""
    if (not SHA.fullmatch(sha) or event.get("repository", {}).get("full_name") != config["repository"]
            or not re.fullmatch(r"develop/[0-9]+\.[0-9]+\.[0-9]+", config["develop_branch"])
            or not re.fullmatch(r"[A-Za-z0-9_-]+\[bot\]", config["release_app_bot_login"])):
        raise Refused("Lightweight CI requires the configured repository and release App")
    repo = config["repository"]
    if mode == "release":
        if os.environ.get("GITHUB_EVENT_NAME") != "pull_request":
            raise Refused("Release CI must verify a pull request merge result")
        announced = event.get("pull_request", {})
        pr = api(f"repos/{repo}/pulls/{int(announced['number'])}")
        metadata = provenance(config, pr)
        if (metadata is None or pr.get("state") != "open" or pr.get("merged_at")
                or pr.get("merge_commit_sha") != sha
                or pr.get("head", {}).get("sha") != announced.get("head", {}).get("sha")):
            raise Refused("Only the current immutable managed release can reuse development CI")
        source = metadata["source_sha"]
        current_candidate(config, source)
        merged = api(f"repos/{repo}/git/commits/{sha}")
        original = api(f"repos/{repo}/git/commits/{source}")
        if merged["tree"]["sha"] != original["tree"]["sha"]:
            raise Refused("The release merge result differs from the tested source tree")
    elif mode == "main":
        if (os.environ.get("GITHUB_EVENT_NAME") not in {"push", "workflow_dispatch"}
                or os.environ.get("GITHUB_REF") != "refs/heads/main"
                or os.environ.get("GITHUB_EVENT_NAME") == "push" and (event.get("ref") != "refs/heads/main" or event.get("after") != sha)):
            raise Refused("Main CI must verify the exact main push or dispatch")
        pr, source = merged_release(config, sha)
        metadata = provenance(config, pr)
        if metadata is None:
            raise Refused("Main must retain the original release App provenance")
        slug = config["release_app_bot_login"][:-5]
        checks = [value for value in pages(f"repos/{repo}/commits/{source}/check-runs", "check_runs")
                  if value.get("name") == CHECK and value.get("app", {}).get("slug") == slug]
        latest = max(checks, key=lambda item: item["id"]) if checks else {}
        if (latest.get("status") != "completed" or latest.get("conclusion") != "success"
                or not re.fullmatch(r"aidash-staging/1:[1-9][0-9]*:[1-9][0-9]*:" + source, latest.get("external_id") or "")):
            raise Refused("Main requires the release App's successful original staging check")
    else:
        raise Refused("Unknown lightweight CI mode")
    completed = source_ci(config, source)
    if completed["id"] != metadata["ci_run_id"]:
        raise Refused("Release provenance does not identify the successful source CI")
    emit("ci.verified", mode=mode, source_sha=source, merge_sha=sha, ci_run_id=completed["id"])


@phase
def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["context", "candidate", "staging", "verify-ci", "reconcile", "attempt"])
    parser.add_argument("--receipt", type=Path)
    args = parser.parse_args()
    if args.action == "attempt":
        if os.environ.get("GITHUB_EVENT_NAME") != "workflow_run":
            raise Refused("Staging intake requires a workflow completion event")
        config = {"repository": os.environ["GITHUB_REPOSITORY"], "develop_branch": os.environ["DEVELOP_BRANCH"]}
        record_attempt(config, json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_text()))
        return
    if args.action == "verify-ci":
        config = {"repository": os.environ["GITHUB_REPOSITORY"], "develop_branch": os.environ.get("DEVELOP_BRANCH", ""),
                  "release_app_bot_login": os.environ.get("RELEASE_APP_BOT_LOGIN", "")}
        event = json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_text())
        verify_ci(config, event, os.environ["GITHUB_SHA"], os.environ["CI_MODE"])
        return
    if os.environ.get("GITHUB_EVENT_NAME") != "workflow_run" and args.action not in {"context", "reconcile"}:
        raise Refused("Release automation only accepts workflow completion events")
    config = configuration()
    event = json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_text())
    emit("workflow.context", action=args.action, token_present=bool(os.environ.get("GH_TOKEN")))
    if args.action == "context":
        for field, variable in [("run_id", "GITHUB_RUN_ID"), ("run_attempt", "GITHUB_RUN_ATTEMPT")]:
            if re.fullmatch(r"[0-9]{1,20}", os.environ.get(variable, "")):
                emit("workflow.identity", **{field: int(os.environ[variable])})
        if SHA.fullmatch(os.environ.get("GITHUB_SHA", "")):
            emit("workflow.identity", controller_sha=os.environ["GITHUB_SHA"])
        return
    if args.action == "reconcile":
        reconcile(config, event)
    elif args.action == "candidate":
        create_candidate(config, event)
    else:
        if args.receipt is not None and args.receipt.stat().st_size > 16384:
            raise Refused("A bounded sanitized receipt artifact is required")
        receipt = json.loads(args.receipt.read_text()) if args.receipt is not None else download_receipt(config, event, optional=True)
        if receipt is not None:
            accept_staging(config, event, receipt)
        else:
            emit("workflow.noop", reason="no_staging_receipt")


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        emit("workflow.failed", error_type=type(error).__name__)
        raise SystemExit(f"Release automation refused: {type(error).__name__}; no private input was logged") from None
