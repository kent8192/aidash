#!/usr/bin/env python3
"""Release candidates and their staging gate, executed only from trusted main.

PR code is never checked out or executed. Every API write uses fixed endpoints
and JSON input; the installation token remains outside deployment artifacts.
"""

import argparse
import io
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import re
import time
from urllib.parse import quote, urlencode
import zipfile

from cloud import run
from policy import IMAGE_KINDS, Refused

MARKER = re.compile(r"<!-- aidash-release:(\{[^\n]*\}) -->")
RELEASE_REF = re.compile(r"release/[0-9]{8}T[0-9]{6}Z-[a-f0-9]{12}")
SHA = re.compile(r"[a-f0-9]{40}")
CHECK = "Staging deployment"
ATTRIBUTION = "🤖 Generated with [Codex](https://openai.com/codex/)"


def api(path, method="GET", data=None):
    args = ["gh", "api", "--method", method, path]
    encoded = None
    if data is not None:
        args.extend(["--input", "-"])
        encoded = json.dumps(data).encode()
    output = run(*args, data=encoded, timeout=60)
    return json.loads(output) if output.strip() else None


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


def completed_run(config, event, filename, name, event_name):
    if event.get("action") != "completed" or event.get("repository", {}).get("full_name") != config["repository"]:
        raise Refused("Release automation requires a same-repository completed workflow event")
    announced = event["workflow_run"]
    value = api(f"repos/{config['repository']}/actions/runs/{int(announced['id'])}")
    owner = workflow(config, filename, name)
    if (
        value.get("id") != announced["id"]
        or value.get("head_sha") != announced.get("head_sha")
        or value.get("workflow_id") != owner["id"]
        or value.get("path") != owner["path"]
        or value.get("name") != name
        or value.get("event") not in ({event_name} if isinstance(event_name, str) else event_name)
        or value.get("status") != "completed"
        or value.get("conclusion") != "success"
        or value.get("repository", {}).get("full_name") != config["repository"]
        or value.get("head_repository", {}).get("full_name") != config["repository"]
        or not SHA.fullmatch(value.get("head_sha", ""))
    ):
        raise Refused("Only the exact successful trusted workflow run can authorize this release")
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


def current_candidate(config, sha):
    if not SHA.fullmatch(sha) or protected_branch(config, config["develop_branch"]) != sha:
        raise Refused("The candidate was superseded by the protected development branch")
    ancestor(config, protected_branch(config, "main"), sha)


def source_ci(config, sha):
    owner = workflow(config, "ci.yml", "CI")
    query = urlencode({"head_sha": sha, "event": "push", "branch": config["develop_branch"]})
    runs = pages(f"repos/{config['repository']}/actions/workflows/ci.yml/runs?{query}", "workflow_runs")
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


def check(config, sha, receipt=None):
    slug = config["release_app_bot_login"][:-5]
    existing = [value for value in pages(f"repos/{config['repository']}/commits/{sha}/check-runs", "check_runs")
                if value.get("name") == CHECK and value.get("app", {}).get("slug") == slug]
    if receipt is None and any(value.get("conclusion") == "success" for value in existing):
        return
    data = {"name": CHECK, "head_sha": sha, "status": "in_progress", "external_id": f"aidash-staging/1:{sha}"}
    if receipt is not None:
        data.update(status="completed", conclusion="success", external_id=f"aidash-staging/1:{receipt['run_id']}:{receipt['generation']}:{receipt['trusted_sha']}",
                    output={"title": "The exact release candidate is ready in staging", "summary": f"Source: `{sha}`\nGeneration: {receipt['generation']}\nTrusted controller: `{receipt['trusted_sha']}`"})
    if existing:
        data.pop("head_sha")
        api(f"repos/{config['repository']}/check-runs/{max(existing, key=lambda item: item['id'])['id']}", "PATCH", data)
    else:
        api(f"repos/{config['repository']}/check-runs", "POST", data)


def create_candidate(config, event):
    completed = completed_run(config, event, "ci.yml", "CI", "push")
    if completed["head_branch"] != config["develop_branch"]:
        return False
    sha = completed["head_sha"]
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
    current_candidate(config, sha)
    for old, _metadata in opened:
        if old["number"] != candidate["number"]:
            api(f"repos/{config['repository']}/pulls/{old['number']}", "PATCH", {"state": "closed"})
    check(config, sha)
    replay_staging(config, sha)
    return True


def download_receipt(config, event, optional=False):
    completed = completed_run(config, event, "gcp-environments.yml", "GCP environments", {"workflow_run", "schedule", "workflow_dispatch", "issue_comment"})
    artifacts = pages(f"repos/{config['repository']}/actions/runs/{completed['id']}/artifacts", "artifacts")
    owned = [value for value in artifacts if value.get("name") == "staging-receipt"]
    if optional and not owned:
        # Scheduled reconciliations and test/PR operations publish no staging evidence.
        return None
    if len(owned) != 1 or owned[0].get("expired") is not False or owned[0].get("workflow_run", {}).get("id") != completed["id"] or not 0 < owned[0].get("size_in_bytes", 0) <= 32768:
        raise Refused("The trusted workflow has no unique bounded staging artifact")
    archive = run("gh", "api", f"repos/{config['repository']}/actions/artifacts/{int(owned[0]['id'])}/zip", timeout=60)
    if len(archive) > 32768:
        raise Refused("Staging artifact exceeded its bounded size")
    with zipfile.ZipFile(io.BytesIO(archive)) as zipped:
        files = zipped.infolist()
        if len(files) != 1 or files[0].filename != "staging-receipt.json" or not 0 < files[0].file_size <= 16384 or files[0].flag_bits & 1:
            raise Refused("Only the sanitized staging receipt can be consumed")
        return json.loads(zipped.read(files[0]))


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
                return
            produced = artifact["workflow_run"]
            if artifact.get("expired") is not False or produced.get("head_branch") != branch:
                continue
            event = {"action": "completed", "repository": {"full_name": config["repository"]},
                     "workflow_run": {"id": produced["id"], "head_sha": produced.get("head_sha")}}
            receipt = download_receipt(config, event)
            if receipt.get("source_sha") == sha:
                accept_staging(config, event, receipt)
                return
        except (Refused, ValueError, KeyError, TypeError, AttributeError, zipfile.BadZipFile):
            # No current trusted acknowledgement means the PR remains a draft.
            continue


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
    main = protected_branch(config, config.get("trusted_branch", "main"))
    ancestor(config, receipt["trusted_sha"], main)
    current_candidate(config, receipt["source_sha"])
    source_ci(config, receipt["source_sha"])
    return receipt


def accept_staging(config, event, receipt):
    validated = validate_receipt(config, event, receipt)
    if validated != download_receipt(config, event):
        raise Refused("Receipt input does not match the original trusted artifact")
    matches = [(pr, meta) for pr, meta in managed_prs(config) if meta["source_sha"] == validated["source_sha"]]
    if len(matches) != 1:
        raise Refused("The receipt must match one current immutable managed release PR")
    pr, metadata = matches[0]
    original = api(f"repos/{config['repository']}/actions/runs/{metadata['ci_run_id']}")
    owner = workflow(config, "ci.yml", "CI")
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
    return True


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["candidate", "staging"])
    parser.add_argument("--receipt", type=Path)
    args = parser.parse_args()
    if os.environ.get("GITHUB_EVENT_NAME") != "workflow_run":
        raise Refused("Release automation only accepts workflow completion events")
    config = configuration()
    event = json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_text())
    if args.action == "candidate":
        create_candidate(config, event)
    else:
        if args.receipt is not None and args.receipt.stat().st_size > 16384:
            raise Refused("A bounded sanitized receipt artifact is required")
        receipt = json.loads(args.receipt.read_text()) if args.receipt is not None else download_receipt(config, event, optional=True)
        if receipt is not None:
            accept_staging(config, event, receipt)


if __name__ == "__main__":
    try:
        main()
    except (Refused, ValueError, KeyError, TypeError, zipfile.BadZipFile) as error:
        raise SystemExit(f"Release automation refused: {type(error).__name__}; no private input was logged") from None
