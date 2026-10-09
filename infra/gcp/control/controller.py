#!/usr/bin/env python3
"""GitHub-authorized nonproduction lifecycle control. Infrastructure only."""

import argparse
from datetime import datetime
import hashlib
import io
import json
import os
from pathlib import Path
import secrets
import subprocess
import tarfile
import time
import urllib.parse
import urllib.request

from cloud import (
    Store,
    Terraform,
    github,
    private_json,
    run,
    OperationDeadline,
    operation_budget,
    bounded_timeout,
    retire_provider_credentials,
)
from policy import (
    Refused,
    attach_release,
    build_needed,
    eligible_base,
    idle_due,
    preview_command,
    transition,
)

ROOT = Path(__file__).resolve().parents[3]
CONFIG = ROOT / "infra/gcp/config.json"


def load_config():
    value = (
        json.loads(CONFIG.read_text())
        if CONFIG.exists()
        else json.loads(os.environ["AIDASH_GCP_CONFIG"])
    )
    required = {
        "project_id",
        "state_bucket",
        "release_bucket",
        "cloudflare_zone_id",
        "deploy_service_account",
        "domain",
        "repository",
        "develop_branch",
    }
    if not required <= value.keys() or value["domain"] != "aidash.run":
        raise Refused(
            "Configure the explicit GCP project, buckets, repository, development branch, and aidash.run zone"
        )
    return value


def permission(config, actor):
    value = github(
        f"repos/{config['repository']}/collaborators/{urllib.parse.quote(actor, safe='')}/permission"
    )
    if value["permission"] not in {"admin", "maintain", "write"}:
        raise Refused("Lifecycle operations require repository write permission")


def ci_success(config, sha):
    query = urllib.parse.urlencode(
        {"head_sha": sha, "status": "success", "per_page": 100}
    )
    value = github(
        f"repos/{config['repository']}/actions/workflows/ci.yml/runs?{query}"
    )
    if not any(
        item["head_sha"] == sha
        and item["conclusion"] == "success"
        and item["status"] == "completed"
        for item in value["workflow_runs"]
    ):
        raise Refused("Required CI has not succeeded for the exact source SHA")


def pull(config, number):
    value = github(f"repos/{config['repository']}/pulls/{int(number)}")
    if value["state"] != "open" or not eligible_base(value["base"]["ref"]):
        raise Refused("PR must be open and target main or develop/x.y.z")
    return value


def source(config, identity, ref, approved_sha=""):
    if identity.startswith("pr-"):
        pr = pull(config, identity[3:])
        sha = pr["head"]["sha"]
        repository = pr["head"]["repo"]["full_name"]
        fork = repository != config["repository"]
        if fork and approved_sha != sha:
            raise Refused(
                "Forks require an explicit approved_sha equal to the current PR head"
            )
        ref = "pr/" + identity[3:]
    else:
        if identity == "develop":
            ref = ref or config["develop_branch"]
            if ref != config["develop_branch"]:
                raise Refused("Shared develop follows the configured develop_branch")
        if not ref:
            raise Refused("Choose a source ref for this test environment")
        repository, fork = config["repository"], False
        sha = github(f"repos/{repository}/commits/{urllib.parse.quote(ref, safe='')}")[
            "sha"
        ]
    ci_success(config, sha)
    return dict(
        sha=sha,
        source_ref=ref,
        source_repo=repository,
        fork=fork,
        approved_sha=approved_sha,
    )


def requests(config, event, name, state):
    sequence = int(os.environ["GITHUB_RUN_ID"])
    common = {"sequence": sequence}
    if name == "workflow_dispatch":
        permission(config, os.environ["GITHUB_ACTOR"])
        inputs = event["inputs"]
        action = inputs["action"]
        identity = (
            "pr-" + inputs["pr_number"]
            if inputs["environment"] == "pr"
            else inputs["environment"]
        )
        value = dict(
            common,
            environment=identity,
            action=action,
            mode=""
            if inputs.get("vm_mode", "default") == "default"
            else inputs["vm_mode"],
            force=inputs.get("force", "false") == "true",
        )
        if action in {"create", "resume"}:
            previous = state.get("environments", {}).get(identity, {})
            ref = inputs.get("source_ref") or previous.get("source_ref", "")
            # Retained test resumes stay pinned unless a new source is explicit.
            if (
                identity == "test"
                and action == "resume"
                and not inputs.get("source_ref")
            ):
                ref = previous.get("sha", "")
            approved = inputs.get("approved_sha") or previous.get("approved_sha", "")
            value.update(source(config, identity, ref, approved))
        return [value]
    if name == "issue_comment":
        if (
            not event.get("issue", {}).get("pull_request")
            or event.get("action") != "created"
        ):
            return []
        command = preview_command(event["comment"]["body"])
        if command is None:
            return []
        permission(config, event["comment"]["user"]["login"])
        identity = "pr-" + str(event["issue"]["number"])
        value = dict(common, environment=identity, **command)
        if command["action"] == "up":
            previous = state.get("environments", {}).get(identity, {})
            approved = command["approved_sha"] or previous.get("approved_sha", "")
            value.update(source(config, identity, "", approved))
        return [value]
    if name != "workflow_run" or event["workflow_run"]["conclusion"] != "success":
        return []
    completed = event["workflow_run"]
    workflow = github(f"repos/{config['repository']}/actions/workflows/ci.yml")
    if completed["workflow_id"] != workflow["id"]:
        raise Refused("Unexpected CI workflow identity")
    candidates = []
    for identity, entry in state.get("environments", {}).items():
        if entry["desired"] == "destroyed" or identity == "test" or entry.get("fork"):
            continue
        if entry["sequence"] == sequence:
            # An accepted workflow-run retry belongs to its original source,
            # even when the followed branch advanced during the failed build.
            candidates.append(
                dict(
                    common,
                    environment=identity,
                    action="update",
                    **{
                        key: entry.get(key, "")
                        for key in (
                            "sha",
                            "source_ref",
                            "source_repo",
                            "fork",
                            "approved_sha",
                        )
                    },
                )
            )
            continue
        try:
            current = source(config, identity, entry["source_ref"])
        except Refused:
            continue
        if current["sha"] == completed["head_sha"]:
            candidates.append(
                dict(common, environment=identity, action="update", **current)
            )
    return candidates


def prepare(config, store):
    event = json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_text())
    state, _ = store.read("lifecycle/state.json")
    state = state or {"environments": {}}
    values = requests(config, event, os.environ["GITHUB_EVENT_NAME"], state)
    prepared = {"build": False, "targets": [], "sha": "", "source_repo": ""}
    if values:
        # Accepting intent and executing cloud effects use the same lock. A stop
        # cannot be acknowledged while an older apply can still start a VM.
        # Refresh source/state after waiting; an earlier read may now be stale.
        with store.lock(wait_seconds=450):
            state, _ = store.read("lifecycle/state.json")
            values = requests(
                config,
                event,
                os.environ["GITHUB_EVENT_NAME"],
                state or {"environments": {}},
            )
            for request in values:

                def accept(current, request=request):
                    updated, changed = transition(
                        current, request, secrets.token_hex(6), time.time()
                    )
                    return updated, (
                        changed,
                        updated["environments"].get(request["environment"]),
                    )

                changed, entry = store.mutate(accept)
                if (
                    entry
                    and (changed or entry["sequence"] == request["sequence"])
                    and build_needed(entry)
                ):
                    # Re-running a failed workflow keeps its run ID. Re-emit
                    # pending builds without changing the accepted generation.
                    prepared.update(
                        build=True, sha=entry["sha"], source_repo=entry["source_repo"]
                    )
                    prepared["targets"].append(
                        {
                            "environment": request["environment"],
                            "generation": entry["generation"],
                        }
                    )
    destination = Path(os.environ.get("RUNNER_TEMP", "/tmp")) / "aidash-request.json"
    private_json(destination, prepared)
    if os.environ.get("GITHUB_OUTPUT"):
        with open(os.environ["GITHUB_OUTPUT"], "a") as file:
            for key in ("build", "sha", "source_repo"):
                file.write(
                    f"{key}={str(prepared[key]).lower() if key == 'build' else prepared[key]}\n"
                )


def update_entry(store, identity, generation, **fields):
    def update(state):
        entry = state["environments"].get(identity)
        if entry and entry["generation"] == generation:
            entry.update(fields)
        return state, None

    store.mutate(update)


def current_entry(store, identity, generation):
    state, _ = store.read("lifecycle/state.json")
    entry = state["environments"].get(identity)
    if not entry or entry["generation"] != generation:
        raise Refused("A newer lifecycle request superseded this operation")
    return entry


def host(config, output, action, force=False):
    # The remote shell receives only a fixed command and an allowlisted action.
    if action not in {
        "observe",
        "health",
        "seal",
        "seal-idle",
        "unseal",
        "keepalive",
        "install",
        "gate",
    }:
        raise Refused("Invalid host action")
    cmd = "sudo python3 /opt/aidash/bootstrap/host.py " + (
        "seal --idle-only" if action == "seal-idle" else action
    )
    if force:
        cmd += " --force"
    return json.loads(
        run(
            "gcloud",
            "compute",
            "ssh",
            output["instance"],
            "--project",
            config["project_id"],
            "--zone",
            output["zone"],
            "--tunnel-through-iap",
            "--quiet",
            "--command",
            cmd,
            timeout=1200 if action == "install" else 90,
        )
    )


def bundle(config, release):
    archive = io.BytesIO()
    files = {
        "host.py": ROOT / "infra/gcp/runtime/host.py",
        "nginx.conf": ROOT / "infra/gcp/runtime/nginx.conf",
        "policy.py": ROOT / "infra/gcp/control/policy.py",
        "control.py": ROOT / "runner/control.py",
        "node_guard.py": ROOT / "runner/node_guard.py",
    }
    with tarfile.open(fileobj=archive, mode="w:gz") as tar:
        for name, data in [
            (name, path.read_bytes()) for name, path in files.items()
        ] + [("release.json", json.dumps(release, sort_keys=True).encode())]:
            item = tarfile.TarInfo(name)
            item.mode = 0o600
            item.size = len(data)
            tar.addfile(item, io.BytesIO(data))
    data = archive.getvalue()
    digest = hashlib.sha256(data).hexdigest()
    path = Path(os.environ.get("RUNNER_TEMP", "/tmp")) / f"{digest}.tar.gz"
    path.write_bytes(data)
    key = f"bundles/{digest}.tar.gz"
    try:
        run(
            "gcloud",
            "storage",
            "cp",
            path,
            f"gs://{config['release_bucket']}/{key}",
            "--if-generation-match=0",
        )
    finally:
        path.unlink(missing_ok=True)
    return key, digest


def instance_status(config, output):
    # List by name instead of treating any describe error as a missing instance.
    value = json.loads(
        run(
            "gcloud",
            "compute",
            "instances",
            "list",
            "--project",
            config["project_id"],
            "--filter",
            "name=" + output["instance"],
            "--format=json(name,status)",
        )
    )
    return value[0]["status"] if value else "MISSING"


def provision_secret(config, output, kind):
    secret = output["runtime_secret"]
    versions = json.loads(
        run(
            "gcloud",
            "secrets",
            "versions",
            "list",
            secret,
            "--project",
            config["project_id"],
            "--filter=state=ENABLED",
            "--format=json(name)",
        )
    )
    if not versions:
        value = os.environ.get("AIDASH_RUNTIME_" + kind.upper())
        if not value:
            raise Refused(
                f"Set the {kind} runtime configuration secret before resuming this environment"
            )
        json.loads(value)
        run(
            "gcloud",
            "secrets",
            "versions",
            "add",
            secret,
            "--project",
            config["project_id"],
            "--data-file=-",
            data=value.encode(),
        )


def restart_bootstrap(config, output, fresh_boot=False):
    ssh = (
        "gcloud",
        "compute",
        "ssh",
        output["instance"],
        "--project",
        config["project_id"],
        "--zone",
        output["zone"],
        "--tunnel-through-iap",
        "--quiet",
        "--command",
    )
    # OS Login readiness says nothing about the boot-time oneshot service.
    for attempt in range(12):
        try:
            run(*ssh, "true", timeout=30)
            break
        except (RuntimeError, TimeoutError, subprocess.TimeoutExpired):
            if attempt == 11:
                raise
            time.sleep(5)
    for _ in range(300):
        value = run(
            *ssh,
            "sudo systemctl show google-startup-scripts.service "
            "--property=ActiveState,Result,ExecMainStatus,"
            "ExecMainStartTimestampMonotonic,ExecMainExitTimestampMonotonic",
            timeout=30,
        )
        state = dict(
            line.split("=", 1) for line in value.decode().splitlines() if "=" in line
        )
        started = int(state["ExecMainStartTimestampMonotonic"])
        finished = int(state["ExecMainExitTimestampMonotonic"])
        if (
            state["ActiveState"] not in {"activating", "deactivating", "reloading"}
            and started > 0
            and finished >= started
        ):
            if (
                fresh_boot
                and state["Result"] == "success"
                and state["ExecMainStatus"] == "0"
            ):
                return
            # An in-place release update needs an explicit run. A fresh boot
            # retries once only after the original script has actually failed.
            # Never retry an SSH timeout around this effect.
            run(
                *ssh,
                "sudo systemctl restart google-startup-scripts.service",
                timeout=1500,
            )
            return
        time.sleep(5)
    raise RuntimeError(
        "startup script did not finish; inspect the host before retrying"
    )


def power(config, output, action):
    if action not in {"start", "stop"}:
        raise Refused("Invalid power operation")
    run(
        "gcloud",
        "compute",
        "instances",
        action,
        output["instance"],
        "--project",
        config["project_id"],
        "--zone",
        output["zone"],
        "--quiet",
        timeout=300,
    )


def public_health(output):
    """Allow bounded DNS/ACME propagation, and reject a previous preview's IP."""
    for attempt in range(24):
        try:
            with urllib.request.urlopen(
                f"https://{output['hostname']}/health", timeout=bounded_timeout(10)
            ) as response:
                value = json.load(response)
                if (
                    response.status == 200
                    and value.get("node_id") == "aidash://" + output["runtime_secret"]
                ):
                    return
        except (OSError, ValueError):
            pass
        if attempt != 23:
            time.sleep(5)
    raise Refused(
        "Public HTTPS did not resolve to this environment with a trusted certificate"
    )


def verify_source(config, identity, entry):
    if (
        identity.startswith("pr-")
        and pull(config, identity[3:])["head"]["sha"] != entry["sha"]
    ):
        raise Refused(
            "PR head changed before deployment; request/await CI for the new SHA"
        )
    ci_success(config, entry["sha"])


def observe_interruptions(config, store, terraform, managed):
    # Observe *all* hosts before any plan. Otherwise refreshing one environment
    # could recreate a missing VM belonging to an environment processed later.
    outputs = terraform.outputs() if managed else {}
    changed = False
    for identity, previous in managed.items():
        status = instance_status(config, outputs[identity])
        if status == "RUNNING" or not previous.get("vm_present", True):
            continue
        if not previous["running"] and status != "MISSING":
            continue
        state, _ = store.read("lifecycle/state.json")
        entry = state["environments"][identity]
        previous.update(running=False, published=False)
        if status == "MISSING":
            previous["vm_present"] = False
        desired = entry["desired"]
        if desired == "running" and not entry.get("start_pending"):
            desired = "stopped"
        update_entry(
            store,
            identity,
            entry["generation"],
            desired=desired,
            status="interrupted",
            published=False,
        )
        changed = True
    return changed


def restore_broker_admission(config, outputs, sealed):
    # Restore every successful preflight seal when reconciliation cannot reach
    # host cleanup, even if an earlier unseal fails. Keep failures explicit.
    failed = False
    for identity in sorted(sealed):
        try:
            host(config, outputs[identity], "unseal")
        except Exception:
            failed = True
    if failed:
        raise RuntimeError("Broker drain admission restoration failed")


def reconcile(config, store):
    with store.lock():
        terraform = Terraform(ROOT / "infra/gcp/environments", config)
        managed = terraform.configuration_in_state(store)
        state, _ = store.read("lifecycle/state.json")
        if not state:
            return
        interrupted = observe_interruptions(config, store, terraform, managed)
        # Observe missing/interrupted VMs before any plan, including broker-only
        # changes. The normal apply fence still forbids unauthorized VM creation.
        broker_changed = terraform.broker_configuration_changed(store, managed)
        presealed = set()
        if broker_changed:
            previous_brokers = terraform.broker_configuration_in_state(store)
            desired_brokers = terraform.broker_configuration(managed)
            outputs = terraform.outputs() if managed else {}
            blocked = False
            # Every apply consumes broker intent, including interruption/lifecycle
            # plans. Drain before any plan can remove or replace a live broker.
            try:
                for identity, previous in managed.items():
                    prior = (previous_brokers or {}).get(identity, {})
                    if not previous["running"] or (
                        previous_brokers is not None
                        and (not prior.get("enabled") or prior == desired_brokers.get(identity))
                    ):
                        continue
                    state, _ = store.read("lifecycle/state.json")
                    entry = state["environments"][identity]
                    current_entry(store, identity, entry["generation"])
                    if host(config, outputs[identity], "seal")["sealed"]:
                        presealed.add(identity)
                    else:
                        update_entry(store, identity, entry["generation"], status="waiting_for_active_work")
                        blocked = True
            except Exception:
                restore_broker_admission(config, outputs, presealed)
                raise
            if blocked:
                restore_broker_admission(config, outputs, presealed)
                return
        if interrupted or broker_changed:
            try:
                terraform.apply(managed)
            except Exception:
                if presealed:
                    restore_broker_admission(config, outputs, presealed)
                raise
        state, _ = store.read("lifecycle/state.json")
        failures = []
        for identity, snapshot in sorted(
            state["environments"].items(),
            key=lambda pair: (pair[1]["desired"] == "running", pair[1]["sequence"]),
        ):
            deployment_started = False
            sealed = identity in presealed
            try:
                generation = snapshot["generation"]
                entry = current_entry(store, identity, generation)
                output = terraform.outputs().get(identity) if managed else None
                # Close/merge cleanup is reconciled regardless of CI, builds, or fork approval.
                if entry["kind"] == "pr" and entry["desired"] != "destroyed":
                    pr = github(f"repos/{config['repository']}/pulls/{identity[3:]}")
                    if pr["state"] != "open":
                        entry["desired"] = "destroyed"
                        update_entry(
                            store,
                            identity,
                            generation,
                            desired="destroyed",
                            status="retiring",
                        )
                if entry["desired"] == "destroyed":
                    current_entry(store, identity, generation)
                    if identity in managed:
                        managed[identity]["published"] = False
                        if config.get("byok_project_id"):
                            status = instance_status(config, output)
                            if status == "RUNNING":
                                # Freeze the only app writer before inventorying
                                # dynamic secrets. Failure keeps the host gated;
                                # retirement never wakes or recreates a VM.
                                if not host(config, output, "seal", force=True)["sealed"]:
                                    raise Refused("Provider Credential writer is not sealed")
                            elif status not in {"MISSING", "TERMINATED", "SUSPENDED"}:
                                raise Refused("Provider Credential writer state is unknown")
                            if status == "MISSING":
                                managed[identity]["vm_present"] = False
                        retire_provider_credentials(config, identity)
                        current_entry(store, identity, generation)
                        terraform.apply(managed)
                        del managed[identity]
                        terraform.apply(managed, retiring={identity})
                    else:
                        # Also recover a partial earlier Terraform retirement.
                        retire_provider_credentials(config, identity)
                    update_entry(
                        store,
                        identity,
                        generation,
                        status="destroyed",
                        applied=None,
                        published=False,
                    )
                    continue
                if entry["desired"] == "stopped":
                    if identity in managed and managed[identity]["running"]:
                        if (
                            not entry.get("force")
                            and not host(config, output, "seal")["sealed"]
                        ):
                            update_entry(store, identity, generation, status="draining")
                            continue
                        current_entry(store, identity, generation)
                        managed[identity]["published"] = False
                        terraform.apply(managed)
                        current_entry(store, identity, generation)
                        power(config, output, "stop")
                        managed[identity]["running"] = False
                        terraform.apply(managed)
                    update_entry(
                        store,
                        identity,
                        generation,
                        status="stopped",
                        applied=managed.get(identity),
                        published=False,
                    )
                    continue
                if entry["kind"] == "pr" and any(
                    value["kind"] == "pr" and value["running"]
                    for key, value in managed.items()
                    if key != identity
                ):
                    update_entry(
                        store, identity, generation, status="waiting_for_pr_slot"
                    )
                    continue
                if entry.get(
                    "failed_deployment_generation"
                ) == generation and not entry.get("start_pending"):
                    continue
                previous = managed.get(identity)
                needs_deploy = bool(entry.get("release")) and (
                    not previous
                    or not previous["running"]
                    or previous.get("release_sha") != entry["sha"]
                    or previous["spot"] != entry["spot"]
                    or not entry.get("applied")
                    or entry["applied"].get("release_sha") != entry["sha"]
                    or (entry.get("force") and entry.get("start_pending"))
                    or entry.get("provider_credentials") != (output or {}).get("provider_credentials")
                )
                if previous and previous["running"]:
                    if entry.get("keepalive_at", 0) > entry.get("keepalive_applied", 0):
                        host(config, output, "keepalive")
                        update_entry(
                            store,
                            identity,
                            generation,
                            keepalive_applied=entry["keepalive_at"],
                        )
                    if needs_deploy and not sealed:
                        if entry.get("force"):
                            try:
                                host(config, output, "gate")
                            except (RuntimeError, subprocess.TimeoutExpired):
                                # Explicit force also permits repair of a host
                                # whose proxy/bootstrap never became available.
                                pass
                        elif not host(config, output, "seal")["sealed"]:
                            update_entry(
                                store,
                                identity,
                                generation,
                                status="waiting_for_active_work",
                            )
                            continue
                        sealed = True
                    elif not needs_deploy:
                        observation = host(config, output, "observe")
                        if idle_due(
                            observation, time.time(), entry.get("keepalive_at", 0)
                        ):
                            if host(config, output, "seal-idle")["sealed"]:
                                sealed = True
                                # A new request wins over this inactivity decision.
                                current_entry(store, identity, generation)
                                update_entry(
                                    store,
                                    identity,
                                    generation,
                                    desired="stopped",
                                    status="idle_stop_requested",
                                )
                                managed[identity]["published"] = False
                                terraform.apply(managed)
                                current_entry(store, identity, generation)
                                power(config, output, "stop")
                                managed[identity]["running"] = False
                                terraform.apply(managed)
                                update_entry(
                                    store,
                                    identity,
                                    generation,
                                    applied=managed[identity],
                                    published=False,
                                    status="stopped",
                                )
                            continue
                        if not entry.get("release"):
                            update_entry(
                                store, identity, generation, status="awaiting_build"
                            )
                            continue
                        if sealed or entry.get("status") != "ready":
                            current_entry(store, identity, generation)
                            host(config, output, "unseal")
                            host(config, output, "health")
                        update_entry(
                            store,
                            identity,
                            generation,
                            status="ready",
                            start_pending=False,
                            provider_credentials=output.get("provider_credentials"),
                        )
                        continue
                if not entry.get("release"):
                    update_entry(store, identity, generation, status="awaiting_build")
                    continue
                # Revalidate source identity at the effect boundary, not just at intake.
                current_entry(store, identity, generation)
                verify_source(config, identity, entry)
                if (not previous or not previous["running"]) and not entry.get(
                    "start_pending"
                ):
                    update_entry(
                        store,
                        identity,
                        generation,
                        desired="stopped",
                        status="interrupted",
                    )
                    continue
                # A manual resume is a single power authorization, not permission
                # to restart forever after a later Spot interruption.
                update_entry(store, identity, generation, start_pending=False)
                deployment_started = True
                key, digest = bundle(config, entry["release"])
                if previous:
                    managed[identity]["published"] = False
                    terraform.apply(managed)
                    # Provisioning mode changes may replace the VM; detach retained disks first.
                    if previous["spot"] != entry["spot"]:
                        power(config, output, "stop")
                        managed[identity]["running"] = False
                        terraform.apply(managed)
                current_entry(store, identity, generation)
                managed[identity] = dict(
                    kind=entry["kind"],
                    generation=generation,
                    incarnation=entry["incarnation"],
                    running=True,
                    published=False,
                    vm_present=True,
                    spot=entry["spot"],
                    bundle_object=key,
                    bundle_sha256=digest,
                    release_sha=entry["sha"],
                )
                fresh_boot = (
                    not previous
                    or not previous["running"]
                    or previous["spot"] != entry["spot"]
                    or not previous.get("vm_present", True)
                )
                terraform.apply(
                    managed,
                    starting={identity} if entry.get("start_pending") else set(),
                )
                output = terraform.outputs()[identity]
                if instance_status(config, output) != "RUNNING":
                    if not entry.get("start_pending"):
                        raise Refused(
                            "Environment stopped during deployment; manual resume is required"
                        )
                    current_entry(store, identity, generation)
                    power(config, output, "start")
                    fresh_boot = True
                provision_secret(config, output, entry["kind"])
                current_entry(store, identity, generation)
                restart_bootstrap(config, output, fresh_boot)
                for attempt in range(24):
                    try:
                        health = host(config, output, "health")
                        break
                    except (RuntimeError, subprocess.TimeoutExpired):
                        if attempt == 23:
                            raise
                        time.sleep(5)
                if health["source_sha"] != entry["sha"]:
                    raise Refused("Running source differs from the authorized release")
                current_entry(store, identity, generation)
                verify_source(config, identity, entry)
                managed[identity]["published"] = True
                terraform.apply(managed)
                current_entry(store, identity, generation)
                host(config, output, "unseal")
                # Certificate issuance requires live DNS; validate it before declaring ready.
                public_health(output)
                current_entry(store, identity, generation)
                verify_source(config, identity, entry)
                update_entry(
                    store,
                    identity,
                    generation,
                    applied=managed[identity],
                    published=True,
                    status="ready",
                    keepalive_applied=entry.get("keepalive_at", 0),
                    provider_credentials=output.get("provider_credentials"),
                )
                print(
                    f"Ready: {identity} https://{output['hostname']} source={entry['sha']}"
                )
            except (Exception, OperationDeadline) as error:
                with operation_budget(180):
                    if deployment_started:
                        # Keep failed releases gated and data intact. A subsequent
                        # explicit resume/new source can retry, but cron cannot loop
                        # migrations or resurrect an interrupted first boot.
                        update_entry(
                            store,
                            identity,
                            snapshot["generation"],
                            failed_deployment_generation=snapshot["generation"],
                        )
                        if identity in managed:
                            managed[identity]["published"] = False
                            try:
                                host(config, terraform.outputs()[identity], "gate")
                            except Exception:
                                pass
                            try:
                                terraform.apply(managed)
                            except Exception:
                                pass
                    elif sealed:
                        try:
                            host(config, output, "unseal")
                        except Exception:
                            pass
                    update_entry(
                        store,
                        identity,
                        snapshot["generation"],
                        status="failed",
                        diagnostic=type(error).__name__,
                    )
                    print(f"Deferred/failed: {identity}: {type(error).__name__}")
                    failures.append(identity)
                if isinstance(error, OperationDeadline):
                    raise
                # A normal failure's cleanup may have consumed the remaining
                # operation budget. Do not start another environment/cleanup.
                bounded_timeout(1)
        if failures:
            raise RuntimeError("Reconciliation incomplete: " + ", ".join(failures))


def finish(config, store, prepared_path, release_path):
    prepared = json.loads(Path(prepared_path).read_text())
    release = json.loads(Path(release_path).read_text())
    for target in prepared["targets"]:

        def attach(state, target=target):
            return attach_release(
                state, target["environment"], target["generation"], release
            )

        store.mutate(attach)
    reconcile(config, store)


def preflight(event, name, repository):
    if name == "issue_comment":
        if event.get("action") != "created" or "pull_request" not in event.get(
            "issue", {}
        ):
            return False
        try:
            if preview_command(event["comment"]["body"]) is None:
                return False
            permission({"repository": repository}, event["comment"]["user"]["login"])
        except Refused:
            return False
    return True


def controller_budget(phase):
    seconds = 8 * 60 if phase == "prepare" else 40 * 60
    if os.environ.get("GITHUB_ACTIONS") == "true":
        # Include runner setup/download time in the allowance. These hard
        # limits match gcp-environments.yml; reserve cleanup + lock release.
        hard_limit, reserve = (
            (10 * 60, 2 * 60) if phase == "prepare" else (55 * 60, 7 * 60)
        )
        with operation_budget(60):
            jobs = github(
                f"repos/{os.environ['GITHUB_REPOSITORY']}/actions/runs/{os.environ['GITHUB_RUN_ID']}/attempts/{os.environ['GITHUB_RUN_ATTEMPT']}/jobs?per_page=100"
            )
        job = next(
            (job for job in jobs["jobs"] if job["name"] == os.environ["GITHUB_JOB"]),
            None,
        )
        if not job or not job.get("started_at"):
            raise Refused("Cannot determine this job's remaining lifecycle time")
        started = datetime.fromisoformat(
            job["started_at"].replace("Z", "+00:00")
        ).timestamp()
        seconds = min(seconds, hard_limit - (time.time() - started) - reserve)
    if seconds < 60:
        raise Refused("Insufficient job time to acquire the lifecycle lock safely")
    return seconds


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "phase", choices=["preflight", "prepare", "finish", "reconcile"]
    )
    parser.add_argument("--request")
    parser.add_argument("--release")
    args = parser.parse_args()
    if args.phase == "preflight":
        event = json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_text())
        proceed = preflight(
            event, os.environ["GITHUB_EVENT_NAME"], os.environ["GITHUB_REPOSITORY"]
        )
        with open(os.environ["GITHUB_OUTPUT"], "a") as output:
            output.write(f"proceed={str(proceed).lower()}\n")
        return
    with operation_budget(controller_budget(args.phase)):
        config = load_config()
        store = Store(config["state_bucket"])
        if args.phase == "prepare":
            prepare(config, store)
        elif args.phase == "finish":
            finish(config, store, args.request, args.release)
        else:
            reconcile(config, store)


if __name__ == "__main__":
    main()
