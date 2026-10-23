#!/usr/bin/env python3
"""Prepare and run a person-supervised, disposable Pre-update Backup drill (#435)."""

import argparse
import datetime as dt
import hashlib
import json
import pathlib
import re
import shutil
import subprocess
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid

ROOT = pathlib.Path(__file__).resolve().parents[2]
MODULE = pathlib.Path(__file__).with_name("restore-drill")
API = "https://gkebackup.googleapis.com/v1/"
BACKUP = re.compile(r"projects/([a-z][a-z0-9-]{4,28}[a-z0-9])/locations/([a-z0-9-]+)/backupPlans/aidash-production-pre-update/backups/(pre-update-g[0-9]+-[a-f0-9]{12})")
CHECKS = {"baseline_high_water", "ledger_preserved", "secrets_reprovisioned", "authorized_read_write", "denied_access", "execution_recovery", "no_duplicate_effects", "event_outbox_replay", "semantic_search", "deleted_sources_absent"}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def read(path):
    return json.loads(pathlib.Path(path).read_text())


def write(path, value):
    path = pathlib.Path(path)
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(value, indent=2) + "\n")
    temporary.chmod(0o600)
    temporary.replace(path)


def digest(path):
    return hashlib.sha256(pathlib.Path(path).read_bytes()).hexdigest()


def call(method, resource, body=None):
    # Never write the access token, Secret contents or HTTP response bodies to logs.
    token = subprocess.check_output(["gcloud", "auth", "print-access-token"], text=True, timeout=30).strip()
    request = urllib.request.Request(API + resource, method=method,
        data=None if body is None else json.dumps(body).encode(),
        headers={"Authorization": "Bearer " + token, "Content-Type": "application/json"})
    with urllib.request.urlopen(request, timeout=60) as response:
        raw = response.read(8 * 1024 * 1024 + 1)
    require(len(raw) <= 8 * 1024 * 1024, "Backup API metadata exceeded the response limit")
    return json.loads(raw or b"{}")


def volume_inventory(backup):
    volumes, cursor = [], ""
    while True:
        page = call("GET", f"{backup}/volumeBackups?" + urllib.parse.urlencode({"pageSize": 1000, "pageToken": cursor}))
        volumes.extend(page.get("volumeBackups", []))
        cursor = page.get("nextPageToken", "")
        if not cursor:
            return volumes


def validate_baseline(entry, settings, backup, volumes):
    name = entry.get("pre_update_backup", "")
    require(BACKUP.fullmatch(name), "Use the dedicated production Pre-update Backup plan")
    require(not entry.get("writers_started"), "Writers started after this backup; use forward repair instead")
    require(backup.get("name") == name and backup.get("state") == "SUCCEEDED", "The exact baseline backup must have succeeded")
    require(backup.get("containsVolumeData") and backup.get("volumeCount", 0) > 0 and backup.get("resourceCount", 0) > 0, "The baseline has no volume/resource data")
    require(not backup.get("permissiveMode", False), "Permissive backups are unsupported")
    require(backup.get("selectedApplications", {}).get("namespacedNames") == [{"namespace": settings["namespace"], "name": "aidash"}], "The baseline must select only Aidash's ProtectedApplication")
    require(not any(field in backup for field in ["allNamespaces", "selectedNamespaces", "excludedNamespaces"]), "Unexpected backup scope")
    require(backup.get("clusterMetadata", {}).get("cluster") == settings["source_cluster"], "The baseline belongs to another cluster")
    require(len(volumes) == backup["volumeCount"] and all(v.get("state") == "SUCCEEDED" for v in volumes), "The complete volume inventory must have succeeded")
    claims = [v.get("sourcePvc") for v in volumes]
    require(all(isinstance(c, dict) and c.get("namespace") == settings["namespace"] and c.get("name") for c in claims), "Unexpected volume scope")
    require({"namespace": settings["namespace"], "name": settings["ledger_claim"]} not in claims, "The baseline contains the independently retained Home ledger")
    return claims


def restore_body(receipt):
    namespace = receipt["settings"]["namespace"]
    # No application code starts merely because Backup for GKE reports success.
    kinds = [("", "Secret"), ("", "Pod"), ("", "ReplicationController"),
             ("apps", "Deployment"), ("apps", "StatefulSet"), ("apps", "ReplicaSet"),
             ("apps", "DaemonSet"), ("batch", "Job"), ("batch", "CronJob")]
    exclusions = [{"groupKind": {"resourceGroup": group, "resourceKind": kind}} for group, kind in kinds]
    exclusions.append({"groupKind": {"resourceGroup": "", "resourceKind": "PersistentVolumeClaim"},
                       "namespace": namespace, "name": receipt["settings"]["ledger_claim"]})
    return {"backup": receipt["backup"], "labels": {"application": "aidash", "lifecycle": "restore-drill"},
            "filter": {"inclusionFilters": [{"groupKind": {"resourceGroup": "", "resourceKind": kind}} for kind in ["PersistentVolumeClaim", "ConfigMap"]], "exclusionFilters": exclusions}}


def same_filter(observed, expected):
    """API JSON may omit default empty core groups or reorder selectors."""
    def selectors(value, key):
        return sorted(json.dumps({"group": row.get("groupKind", {}).get("resourceGroup", ""),
                                  "kind": row.get("groupKind", {}).get("resourceKind", ""),
                                  "name": row.get("name", ""), "namespace": row.get("namespace", ""),
                                  "labels": row.get("labels", {})}, sort_keys=True)
                      for row in value.get(key, []))
    return all(selectors(observed, key) == selectors(expected, key) for key in ["inclusionFilters", "exclusionFilters"])


def prepare(directory, entry, settings):
    require(not directory.exists(), "Choose a new drill directory; never reuse another drill's state")
    require(directory.resolve().is_relative_to((ROOT / "private").resolve()), "Keep drill state and evidence in this checkout's private/ directory")
    required = {"billing_account", "folder_id", "region", "client_cidr", "namespace", "ledger_claim", "source_cluster"}
    require(set(settings) == required and all(isinstance(v, str) and v for v in settings.values()), "Supply exactly the documented drill settings")
    backup = entry.get("pre_update_backup", "")
    require(BACKUP.fullmatch(backup), "Invalid Pre-update Backup name")
    observed = call("GET", backup)
    claims = validate_baseline(entry, settings, observed, volume_inventory(backup))
    identifier = uuid.uuid4().hex[:12]
    variables = {k: settings[k] for k in ["billing_account", "folder_id", "region", "client_cidr", "namespace"]}
    variables.update(project_id="aidash-drill-" + identifier, backup_plan=backup.rsplit("/backups/", 1)[0])
    directory.mkdir(parents=True, mode=0o700)
    for name in ["main.tf", ".terraform.lock.hcl"]:
        shutil.copy2(MODULE / name, directory / name)
    write(directory / "terraform.tfvars.json", variables)
    receipt = {"id": identifier, "project_id": variables["project_id"], "backup": backup,
               "settings": settings, "baseline": {"writers_started": False, "volume_claims": claims},
               "prepared_at": dt.datetime.now(dt.UTC).isoformat(), "phase": "prepared",
               "inputs_sha256": {name: digest(directory / name) for name in ["main.tf", ".terraform.lock.hcl", "terraform.tfvars.json"]}}
    write(directory / "receipt.json", receipt)
    return receipt


def load(directory):
    receipt = read(directory / "receipt.json")
    require(receipt["project_id"] == "aidash-drill-" + receipt["id"] and re.fullmatch(r"[a-f0-9]{12}", receipt["id"]), "Invalid drill ownership receipt")
    require(all(digest(directory / name) == expected for name, expected in receipt["inputs_sha256"].items()), "Drill Terraform inputs changed; retain the reviewed inputs")
    require(read(directory / "terraform.tfvars.json")["project_id"] == receipt["project_id"], "The project no longer belongs to this drill")
    return receipt


def terraform(directory, *args):
    subprocess.run(["terraform", f"-chdir={directory}", *args], check=True)


def outputs(directory, receipt):
    value = json.loads(subprocess.check_output(["terraform", f"-chdir={directory}", "output", "-json", "drill"], text=True))
    prefix = "projects/" + receipt["project_id"] + "/locations/" + receipt["settings"]["region"]
    require(value == {"project_id": receipt["project_id"], "cluster": prefix + "/clusters/restore-drill", "restore_plan": prefix + "/restorePlans/pre-update-drill"}, "Terraform outputs do not name this drill's isolated cluster")
    return value


def restore(directory, receipt):
    target = outputs(directory, receipt)
    validate_baseline({"pre_update_backup": receipt["backup"]}, receipt["settings"], call("GET", receipt["backup"]), volume_inventory(receipt["backup"]))
    plan = call("GET", target["restore_plan"])
    require(plan.get("cluster") == target["cluster"] and plan.get("backupPlan") == receipt["backup"].rsplit("/backups/", 1)[0], "Restore plan destination/source drifted")
    config = plan.get("restoreConfig", {})
    require(config.get("namespacedResourceRestoreMode") == "MERGE_SKIP_ON_CONFLICT" and config.get("volumeDataRestorePolicy") == "RESTORE_VOLUME_DATA_FROM_BACKUP" and config.get("selectedNamespaces", {}).get("namespaces") == [receipt["settings"]["namespace"]] and config.get("clusterResourceRestoreScope", {}).get("noGroupKinds") is True, "Restore plan scope drifted")
    name = target["restore_plan"] + "/restores/drill-" + receipt["id"]
    body = restore_body(receipt)
    receipt.update(restore=name, phase="restoring")
    write(directory / "receipt.json", receipt)
    try:
        call("POST", target["restore_plan"] + "/restores?restoreId=drill-" + receipt["id"], body)
    except urllib.error.HTTPError as error:
        if error.code != 409:
            raise
    deadline = time.monotonic() + 1800
    while time.monotonic() < deadline:
        result = call("GET", name)
        require(result.get("backup") == body["backup"] and same_filter(result.get("filter", {}), body["filter"]), "Existing restore has different exclusions or baseline")
        if result.get("state") == "SUCCEEDED":
            require(result.get("resourcesFailedCount", 0) == 0 and result.get("volumesRestoredCount", 0) > 0, "No successful data-volume restore")
            receipt.update(phase="resources_restored", restore_completed_at=result.get("completeTime"), application_verified=False)
            write(directory / "receipt.json", receipt)
            return
        require(result.get("state") not in {"FAILED", "DELETING"}, "Restore failed; keep its metadata for investigation")
        time.sleep(10)
    raise ValueError("Restore exceeded 30 minutes; inspect before retry or cleanup")


def record(directory, receipt, evidence):
    require(receipt["phase"] == "resources_restored", "Complete the isolated data restore first")
    require(set(evidence) == {"performed_by", "revision", "image_digests", "fault_at", "verified_at", "checks", "report"}, "Use the documented human evidence fields")
    require(set(evidence["checks"]) == CHECKS and all(type(v) is bool for v in evidence["checks"].values()), "Record every acceptance check as true or false")
    require(all(evidence[k] for k in ["performed_by", "revision", "image_digests", "report"]), "Name the person running the drill, revision/images and raw report")
    start, end = [dt.datetime.fromisoformat(evidence[k]) for k in ["fault_at", "verified_at"]]
    require(start.tzinfo and end.tzinfo and end >= start, "Use ordered timestamps with time zones")
    seconds = (end - start).total_seconds()
    passed = all(evidence["checks"].values()) and seconds <= 7200
    write(directory / "human-evidence.json", evidence)
    receipt.update(application_verified=passed, measured_rto_seconds=seconds, human_evidence_sha256=digest(directory / "human-evidence.json"))
    write(directory / "receipt.json", receipt)
    print("Human acceptance:", "passed" if passed else "failed")


def destroy(directory, receipt):
    if receipt.get("restore"):
        name = receipt["restore"]
        require(name.startswith(f"projects/{receipt['project_id']}/"), "Refuse to delete any source-project restore")
        try:
            call("DELETE", name)
        except urllib.error.HTTPError as error:
            if error.code != 404:
                raise
        deadline = time.monotonic() + 1800
        while True:
            try:
                call("GET", name)
            except urllib.error.HTTPError as error:
                if error.code == 404:
                    break
                raise
            require(time.monotonic() < deadline, "Restore deletion is still pending; rerun cleanup")
            time.sleep(10)
    terraform(directory, "plan", "-destroy", "-out=destroy.tfplan", "-input=false")
    terraform(directory, "apply", "-input=false", "destroy.tfplan")
    receipt["phase"] = "destroyed"
    write(directory / "receipt.json", receipt)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["prepare", "apply", "restore", "record", "destroy"])
    parser.add_argument("--directory", type=pathlib.Path, required=True)
    parser.add_argument("--entry", type=pathlib.Path)
    parser.add_argument("--settings", type=pathlib.Path)
    parser.add_argument("--evidence", type=pathlib.Path)
    args = parser.parse_args()
    directory = args.directory.resolve()
    if args.action == "prepare":
        require(args.entry and args.settings, "prepare needs --entry and --settings")
        receipt = prepare(directory, read(args.entry), read(args.settings))
        print("Prepared isolated project:", receipt["project_id"])
        return
    receipt = load(directory)
    if args.action == "apply":
        terraform(directory, "init", "-input=false", "-lockfile=readonly")
        terraform(directory, "plan", "-out=drill.tfplan", "-input=false")
        terraform(directory, "apply", "-input=false", "drill.tfplan")
    elif args.action == "restore":
        restore(directory, receipt)
    elif args.action == "record":
        require(args.evidence, "record needs --evidence")
        record(directory, receipt, read(args.evidence))
    else:
        destroy(directory, receipt)


if __name__ == "__main__":
    try:
        main()
    except (ValueError, KeyError, subprocess.CalledProcessError, urllib.error.URLError) as error:
        raise SystemExit(str(error)) from error
