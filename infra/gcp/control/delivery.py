"""Trusted release authorization and immutable deployment evidence. No mail credentials."""

from copy import deepcopy
import hashlib
import io
import json
import os
import re
import time
import urllib.error
import urllib.parse
import zipfile

from cloud import Store, github, run, trusted_revision
from policy import IMAGE_KINDS, Refused

SHA = re.compile(r"[a-f0-9]{40}")
TOKEN = re.compile(r"[a-f0-9]{32}")
RELEASE_MARKER = re.compile(r"<!-- aidash-release:(\{[^\n]+\}) -->")
PENDING_PRODUCTION = {"requested", "draining", "waiting_for_active_work", "waiting_for_gcip_refresh"}


def production(config):
    return config.get("deployment_tier", "nonproduction") == "production"


def dispatch_production(config, event):
    """Credential-free bridge: workflow_run executes on the default branch, WIF on main."""
    completed = event.get("workflow_run", {})
    repo = config["repository"]
    workflow = github(f"repos/{repo}/actions/workflows/ci.yml")
    if not completed and os.environ.get("GITHUB_EVENT_NAME") == "schedule":
        branch = github(f"repos/{repo}/branches/main")
        sha = branch["commit"]["sha"]
        runs = github(f"repos/{repo}/actions/workflows/ci.yml/runs?head_sha={sha}&event=push&status=success&per_page=100")
        completed = next((item for item in runs.get("workflow_runs", []) if item.get("head_sha") == sha and item.get("head_branch") == "main" and item.get("status") == "completed" and item.get("conclusion") == "success"), {})
        if not completed:
            return False
    if (completed.get("workflow_id") != workflow["id"] or completed.get("event") != "push"
            or completed.get("head_branch") != "main" or completed.get("conclusion") != "success"
            or completed.get("status") != "completed"
            or completed.get("head_repository", {}).get("full_name") != repo):
        raise Refused("Only a completed official main CI run may dispatch production")
    sha = completed.get("head_sha", "")
    branch = github(f"repos/{repo}/branches/main")
    if not SHA.fullmatch(sha) or not branch.get("protected"):
        raise Refused("The production branch must be protected")
    if branch["commit"]["sha"] != sha:
        return False
    pending = github(f"repos/{repo}/actions/workflows/gcp-production.yml/runs?event=workflow_dispatch&head_sha={sha}&per_page=100")
    if any(item.get("status") in {"queued", "in_progress", "waiting", "pending", "requested"} for item in pending.get("workflow_runs", [])):
        return False
    previous = [item for item in pending.get("workflow_runs", []) if item.get("status") == "completed"]
    if previous and read_production_progress(config, max(previous, key=lambda item: int(item["id"])), sha) not in PENDING_PRODUCTION:
        # No terminal deployment or uncertain failure may create repeated approvals.
        return False
    actual = github(f"repos/{repo}/actions/runs/{int(completed['id'])}")
    if any(actual.get(key) != completed.get(key) for key in ("workflow_id", "head_sha", "head_branch", "event", "conclusion", "status")):
        raise Refused("Main CI event differs from the authoritative run")
    run("gh", "workflow", "run", "gcp-production.yml", "--repo", repo, "--ref", "main", "--field", "main_sha=" + sha)
    return True


def read_production_progress(config, completed, sha):
    """Consume only a bounded safe artifact from this protected main workflow."""
    if (completed.get("conclusion") != "success" or completed.get("head_sha") != sha
            or completed.get("head_branch") != "main" or completed.get("event") != "workflow_dispatch"):
        return None
    artifacts = github(f"repos/{config['repository']}/actions/runs/{int(completed['id'])}/artifacts?per_page=100")
    owned = [item for item in artifacts.get("artifacts", []) if item.get("name") == "production-progress"]
    if (len(owned) != 1 or owned[0].get("expired") is not False
            or owned[0].get("workflow_run", {}).get("id") != completed["id"]
            or not 0 < owned[0].get("size_in_bytes", 0) <= 4096):
        return None
    raw = run("gh", "api", f"repos/{config['repository']}/actions/artifacts/{int(owned[0]['id'])}/zip", timeout=60)
    if len(raw) > 4096:
        return None
    try:
        with zipfile.ZipFile(io.BytesIO(raw)) as archive:
            files = archive.infolist()
            if len(files) != 1 or files[0].filename != "production-progress.json" or not 0 < files[0].file_size <= 1024 or files[0].flag_bits & 1:
                return None
            value = json.loads(archive.read(files[0]))
        if (not isinstance(value, dict) or set(value) != {"protocol", "main_sha", "status"}
                or value["protocol"] != "aidash-production-progress/1" or value["main_sha"] != sha
                or value["status"] not in PENDING_PRODUCTION | {"ready", "failed", "interrupted"}):
            return None
        return value["status"]
    except (ValueError, TypeError, zipfile.BadZipFile, RuntimeError):
        return None


def export_production_progress(config, store, main_sha):
    from cloud import private_json
    state, _ = store.read("lifecycle/state.json")
    entry = (state or {}).get("environments", {}).get("production", {})
    if not production(config) or entry.get("main_sha") != main_sha or entry.get("status") not in PENDING_PRODUCTION | {"ready", "failed", "interrupted"}:
        return False
    private_json(os.path.join(os.environ["RUNNER_TEMP"], "production-progress.json"),
                 {"protocol": "aidash-production-progress/1", "main_sha": main_sha, "status": entry["status"]})
    return True


def validate_config(config):
    tier = config.get("deployment_tier", "nonproduction")
    if tier not in {"nonproduction", "production"}:
        raise Refused("Unknown deployment tier")
    if not production(config):
        return
    for key in ("nonproduction_project_id", "nonproduction_state_bucket", "staging_release_bucket",
                "staging_project_id", "notification_bucket", "release_app_bot_login"):
        if not config.get(key):
            raise Refused("Production requires explicit isolation and release authorization inputs")
    if (config["project_id"] == config["nonproduction_project_id"]
            or config["state_bucket"] == config["nonproduction_state_bucket"]
            or config["staging_project_id"] != config["nonproduction_project_id"]
            or config["staging_release_bucket"] in {config["state_bucket"], config["release_bucket"], config["notification_bucket"]}
            or len({config["state_bucket"], config["release_bucket"], config["notification_bucket"]}) != 3):
        raise Refused("Production and nonproduction projects and state must be isolated")
    origin = urllib.parse.urlsplit(config.get("production_origin", ""))
    if (origin.scheme != "https" or origin.netloc != origin.hostname or origin.path
            or origin.query or origin.fragment or origin.hostname not in {"aidash.run", "www.aidash.run"}):
        raise Refused("Use an explicit HTTPS production origin without redirects or credentials")


def hostname(config, identity, entry):
    if identity == "production":
        return urllib.parse.urlsplit(config["production_origin"]).hostname
    if identity == "develop" or identity.startswith("pr-"):
        token = entry.get("hostname_token", "")
        if not TOKEN.fullmatch(token):
            raise Refused("Preview hostname must have a controller-issued cryptographic token")
        return token + ".preview.aidash.run"
    if identity == "test":
        return "test.aidash.run"
    raise Refused("Unknown deployment identity")


def valid_images(images, project):
    if set(images) != set(IMAGE_KINDS):
        raise Refused("Release evidence must name the complete image bundle")
    pattern = re.compile(r"us-central1-docker\.pkg\.dev/" + re.escape(project)
                         + r"/aidash/[a-z0-9-]+@sha256:[a-f0-9]{64}")
    if not all(isinstance(value, str) and pattern.fullmatch(value) for value in images.values()):
        raise Refused("Release evidence contains an unapproved registry or mutable image")


def authorize_production(config, main_sha, ci_check, accepted=None):
    """Bind a human-merged release, main CI and staging artifacts to the same tree."""
    validate_config(config)
    if not production(config) or not SHA.fullmatch(main_sha):
        raise Refused("Production needs an exact main SHA")
    repo = config["repository"]
    branch = github(f"repos/{repo}/branches/main")
    if not branch.get("protected") or branch["commit"]["sha"] != main_sha:
        raise Refused("Production must deploy the current protected main SHA")
    ci_check(dict(config, required_ci_branch="main"), main_sha)
    pulls = github(f"repos/{repo}/commits/{main_sha}/pulls?per_page=100")
    candidates = []
    for pr in pulls:
        if not isinstance(pr.get("number"), int):
            continue
        pr = github(f"repos/{repo}/pulls/{pr['number']}")
        match = RELEASE_MARKER.search(pr.get("body") or "")
        if not match:
            continue
        try:
            metadata = json.loads(match.group(1))
            source = metadata["source_sha"]
        except (ValueError, KeyError, TypeError):
            continue
        if not isinstance(source, str) or not SHA.fullmatch(source):
            continue
        if (metadata.get("protocol") != "aidash-release/1"
                or metadata.get("source_branch") != config["develop_branch"]
                or pr.get("merge_commit_sha") != main_sha or not pr.get("merged_at")
                or pr.get("base", {}).get("ref") != "main"
                or pr.get("head", {}).get("repo", {}).get("full_name") != repo
                or pr.get("head", {}).get("sha") != source
                or not re.fullmatch(r"release/[0-9]{8}T[0-9]{6}Z-" + source[:12], pr["head"].get("ref", ""))
                or pr.get("user", {}).get("login") != config["release_app_bot_login"]
                or pr.get("merged_by", {}).get("type") != "User"):
            continue
        permission = github(f"repos/{repo}/collaborators/{urllib.parse.quote(pr['merged_by']['login'], safe='')}/permission")
        if permission.get("permission") not in {"admin", "maintain", "write"}:
            continue
        candidates.append((pr, source))
    if len(candidates) != 1:
        raise Refused("Main must be a human-merged managed release PR")
    pr, source = candidates[0]
    main_commit = github(f"repos/{repo}/git/commits/{main_sha}")
    source_commit = github(f"repos/{repo}/git/commits/{source}")
    if main_commit["tree"]["sha"] != source_commit["tree"]["sha"]:
        raise Refused("Main differs from the staging-tested tree; synchronize and revalidate")
    parents = main_commit.get("parents", [])
    if not parents or github(f"repos/{repo}/compare/{parents[0]['sha']}...{source}").get("status") not in {"ahead", "identical"}:
        raise Refused("The candidate does not include the previous main history")
    ci_check(dict(config, required_ci_branch=config["develop_branch"]), source)
    if accepted is not None:
        # Only the protected lifecycle state supplies an already admitted tuple.
        # Recheck current main/human provenance above, without reading a mutable
        # staging index that cannot revoke or replace this accepted bundle.
        if (not isinstance(accepted, dict) or set(accepted) != {"source_sha", "trusted_sha", "images"}
                or accepted["source_sha"] != source or not SHA.fullmatch(accepted["trusted_sha"])
                or github(f"repos/{repo}/compare/{accepted['trusted_sha']}...{main_sha}").get("status") not in {"ahead", "identical"}):
            raise Refused("The accepted bundle has invalid main provenance")
        valid_images(accepted["images"], config["staging_project_id"])
        return {"sha": source, "main_sha": main_sha, "source_ref": "main", "source_repo": repo,
                "fork": False, "approved_sha": "", "release_pr": pr["number"], "release": deepcopy(accepted)}
    receipt, _ = Store(config["staging_release_bucket"]).read(f"deployments/develop/{source}.json")
    if not receipt or receipt.get("protocol") != "aidash-staging-receipt/1" or receipt.get("status") != "ready" or receipt.get("source_sha") != source:
        raise Refused("No successful staging evidence exists for this exact release")
    trusted = receipt.get("trusted_sha", "")
    if not SHA.fullmatch(trusted) or github(f"repos/{repo}/compare/{trusted}...{main_sha}").get("status") not in {"ahead", "identical"}:
        raise Refused("The staged controller is not an approved main ancestor")
    workflow = github(f"repos/{repo}/actions/workflows/gcp-environments.yml")
    stage_run = github(f"repos/{repo}/actions/runs/{int(receipt['run_id'])}")
    if (stage_run.get("workflow_id") != workflow["id"] or stage_run.get("conclusion") != "success"
            or stage_run.get("status") != "completed"
            or stage_run.get("head_branch") != config.get("automation_branch", config["develop_branch"])):
        raise Refused("Staging evidence is not from a completed trusted deployment")
    if not 0 <= time.time() - receipt.get("occurred_at", 0) <= config.get("staging_max_age_seconds", 604800):
        raise Refused("Staging evidence expired; revalidate the candidate")
    valid_images(receipt.get("images", {}), config["staging_project_id"])
    return {"sha": source, "main_sha": main_sha, "source_ref": "main", "source_repo": repo,
            "fork": False, "approved_sha": "", "release_pr": pr["number"],
            "release": {"source_sha": source, "trusted_sha": trusted, "images": receipt["images"]}}


def immutable_write(store, key, value):
    try:
        store.write(key, value, "0")
    except urllib.error.HTTPError as error:
        if error.code != 412:
            raise
        previous, _ = store.read(key)
        if previous != value:
            raise Refused("Immutable deployment evidence already differs") from error


def publish_result(config, store, identity, generation, output, status):
    """Queue once from controller state, never from an application's reported status."""
    if not config.get("notification_bucket") and not config.get("release_flow_enabled"):
        return
    state, _ = store.read("lifecycle/state.json")
    entry = state["environments"][identity]
    if entry["generation"] != generation or entry.get("status") != status or status not in {"ready", "failed"}:
        raise Refused("Only the current verified deployment outcome may be published")
    key = f"{identity}:{entry['incarnation']}:{generation}:{entry['sha']}:{status}:{entry.get('outcome_version', 0)}"
    event_id = hashlib.sha256(key.encode()).hexdigest()
    timestamp = int(entry.get("outcome_at") or time.time())
    # Persist the timestamp before external writes so retries produce identical receipts.
    def remember(current):
        target = current["environments"].get(identity, {})
        if target.get("generation") != generation or target.get("status") != status:
            raise Refused("Deployment outcome was superseded before publication")
        target.setdefault("outcome_at", timestamp)
        target.setdefault("outcome_run_id", str(os.environ["GITHUB_RUN_ID"]))
        target.setdefault("outcome_trusted_sha", trusted_revision())
        return current, (target["outcome_at"], target["outcome_run_id"], target["outcome_trusted_sha"])
    timestamp, run_id, outcome_trusted = store.mutate(remember)
    receipt = {"protocol": "aidash-deployment-notification/1", "event_id": event_id,
               "environment": identity, "incarnation": entry["incarnation"], "generation": generation,
               "source_sha": entry["sha"], "trusted_sha": outcome_trusted, "status": status,
               "hostname": hostname(config, identity, entry), "run_id": run_id,
               "occurred_at": timestamp}
    if config.get("notification_bucket"):
        immutable_write(Store(config["notification_bucket"]), f"notifications/{event_id}.json", receipt)
    if identity == "develop" and status == "ready" and config.get("release_flow_enabled"):
        validation_run = int(entry.get("staging_publication_run_id", run_id))
        if validation_run != int(os.environ["GITHUB_RUN_ID"]):
            raise Refused("Staging evidence requires trusted readiness revalidation in this run")
        validation_time = entry.get("staging_verified_at", timestamp)
        staged = {"protocol": "aidash-staging-receipt/1", "source_sha": entry["sha"],
                  "trusted_sha": entry["release"]["trusted_sha"], "generation": generation,
                  "status": status, "images": deepcopy(entry["release"]["images"]),
                  "run_id": validation_run, "occurred_at": validation_time}
        releases = Store(config["release_bucket"])
        immutable_write(releases, f"deployments/develop/{entry['sha']}/{event_id}-{validation_run}-{validation_time}.json", staged)
        index = f"deployments/develop/{entry['sha']}.json"
        for _ in range(5):
            previous, version = releases.read(index)
            if previous and (previous == staged or previous.get("occurred_at", 0) > validation_time):
                break
            try:
                releases.write(index, staged, version)
                break
            except urllib.error.HTTPError as error:
                if error.code != 412:
                    raise
        else:
            raise Refused("Staging evidence index changed repeatedly")


def export_staging_receipt(config, store):
    from cloud import private_json
    state, _ = store.read("lifecycle/state.json")
    entry = (state or {}).get("environments", {}).get("develop", {})
    if entry.get("status") != "ready" or not config.get("release_flow_enabled"):
        return False
    receipt, _ = Store(config["release_bucket"]).read(f"deployments/develop/{entry['sha']}.json")
    if not receipt or str(receipt.get("run_id")) != os.environ["GITHUB_RUN_ID"]:
        return False
    receipt = {key: receipt[key] for key in ("protocol", "source_sha", "trusted_sha", "generation", "status", "images", "run_id")}
    receipt["run_id"] = int(receipt["run_id"])
    private_json(os.path.join(os.environ["RUNNER_TEMP"], "staging-receipt.json"), receipt)
    return True


def flush_outcomes(config, store):
    """Retry evidence/outbox publication without changing a successful deployment."""
    state, _ = store.read("lifecycle/state.json")
    failures = []
    for identity, entry in (state or {}).get("environments", {}).items():
        stage_pending = identity == "develop" and config.get("release_flow_enabled") and entry.get("staging_publication_pending")
        if entry.get("status") not in {"ready", "failed"} or entry.get("outcome_published") == entry.get("status") and not stage_pending or not entry.get("outcome_at"):
            continue
        try:
            publish_result(config, store, identity, entry["generation"], None, entry["status"])
            def ack(current, identity=identity, generation=entry["generation"], status=entry["status"]):
                target = current["environments"].get(identity, {})
                if target.get("generation") == generation and target.get("status") == status:
                    target["outcome_published"] = status
                    target.pop("staging_publication_pending", None)
                return current, None
            store.mutate(ack)
        except Exception:
            failures.append(identity)
    if failures:
        raise RuntimeError("Deployment evidence publication pending; deployment state is preserved")


def staging_publication_complete(config, entry):
    """Mail acknowledgements do not prove a successful uploaded staging artifact."""
    receipt, _ = Store(config["release_bucket"]).read(f"deployments/develop/{entry['sha']}.json")
    if not receipt or time.time() - receipt.get("occurred_at", 0) > config.get("staging_max_age_seconds", 604800):
        return False
    repo, run_id = config["repository"], int(receipt["run_id"])
    completed = github(f"repos/{repo}/actions/runs/{run_id}")
    workflow = github(f"repos/{repo}/actions/workflows/gcp-environments.yml")
    if (completed.get("status") != "completed" or completed.get("conclusion") != "success"
            or completed.get("workflow_id") != workflow["id"]
            or completed.get("head_branch") != config.get("automation_branch", config["develop_branch"])):
        return False
    artifacts = github(f"repos/{repo}/actions/runs/{run_id}/artifacts?per_page=100")
    owned = [item for item in artifacts.get("artifacts", []) if item.get("name") == "staging-receipt"]
    return len(owned) == 1 and owned[0].get("expired") is False and owned[0].get("workflow_run", {}).get("id") == run_id


def install_tls(config, cluster, identity, output):
    if not re.fullmatch(r"[a-f0-9]{32}\.preview\.aidash\.run", output.get("hostname", "")):
        return
    secret = config.get("preview_tls_secret", "")
    if not re.fullmatch(r"[A-Za-z0-9_-]{1,255}", secret):
        raise Refused("Configure the trusted wildcard TLS secret before preview deployment")
    data = json.loads(run("gcloud", "secrets", "versions", "access", "latest", "--secret", secret,
                          "--project", config["project_id"]))
    import kube
    kube.install_preview_tls(cluster, identity, data["certificate_pem"], data["private_key_pem"])
