"""Pre-update Backup: an on-demand Backup for GKE backup taken while writers are sealed.

A person restores it only for an update that failed before `start_writers`
(`restorable`). The ledger claim is left out by the ProtectedApplication.
"""

import json
import re
import time
import urllib.error
import urllib.request

from cloud import DEADLINE, bounded_timeout, run
from policy import Refused

API = "https://gkebackup.googleapis.com/v1/"
PLAN = re.compile(r"projects/[a-z][a-z0-9-]{4,28}[a-z0-9]/locations/[a-z0-9-]+/backupPlans/[a-z][a-z0-9-]{0,62}")
APPLICATION = [{"namespace": "aidash-production", "name": "aidash"}]
MAX_WAIT = 30 * 60
DEPLOY_RESERVE = 20 * 60
POLL = 15
# A Backup created a moment ago may not be readable yet.
NOT_FOUND_GRACE = 60


def application_scope(backup):
    """The completed Backup selected exactly the protected application."""
    return (
        backup.get("selectedApplications", {}).get("namespacedNames") == APPLICATION
        and not any(
            field in backup
            for field in ("selectedNamespaces", "excludedNamespaces", "allNamespaces")
        )
    )


def call(method, resource, body=None):
    token = run("gcloud", "auth", "print-access-token", timeout=30).decode().strip()
    request = urllib.request.Request(
        API + resource,
        data=None if body is None else json.dumps(body).encode(),
        method=method,
        headers={"Authorization": "Bearer " + token, "Content-Type": "application/json"},
    )
    with urllib.request.urlopen(request, timeout=bounded_timeout(60)) as response:
        return json.loads(response.read() or b"{}")


def backup_id(generation, sha):
    return f"pre-update-g{generation}-{sha[:12]}"


def restorable(entry):
    """True while no writer has run since the backup, so a restore loses no acknowledged write."""
    return bool(entry.get("pre_update_backup")) and not entry.get("writers_started")


def wait_window():
    window = MAX_WAIT
    deadline = DEADLINE.get()
    if deadline is not None:
        window = min(window, deadline - time.monotonic() - DEPLOY_RESERVE)
    if window <= 0:
        raise Refused("Too little of the reconcile budget remains for a Pre-update Backup and the deployment")
    return window


def take(plan, generation, sha):
    """Create the generation's Backup, or resume it after a crashed run, and wait for SUCCEEDED."""
    if not PLAN.fullmatch(plan):
        raise Refused("The Pre-update Backup plan name is invalid")
    identifier = backup_id(generation, sha)
    name = f"{plan}/backups/{identifier}"
    give_up = time.monotonic() + wait_window()
    created = time.monotonic()
    try:
        call("POST", f"{plan}/backups?backupId={identifier}", {})
    except urllib.error.HTTPError as error:
        # The same generation already created it: poll the existing Backup.
        if error.code != 409:
            raise
    while True:
        try:
            backup = call("GET", name)
        except urllib.error.HTTPError as error:
            if error.code != 404:
                raise
            if time.monotonic() - created >= NOT_FOUND_GRACE:
                raise Refused("The Pre-update Backup is missing") from error
            backup = {}
        state = backup.get("state")
        if state == "SUCCEEDED":
            # A scope that selected nothing also succeeds; that is no backup.
            if not backup.get("containsVolumeData") or not backup.get("resourceCount") or not backup.get("volumeCount"):
                raise Refused("The Pre-update Backup contains no volume data")
            # Without Secrets the restore cannot recreate the runtime
            # configuration the application needs, so it is not a restore point.
            if not backup.get("containsSecrets"):
                raise Refused("The Pre-update Backup contains no Secrets")
            # A plan that drifted to another application or namespace scope
            # would still produce a SUCCEEDED backup; it is not ours to restore.
            if not application_scope(backup):
                raise Refused("The Pre-update Backup does not select the protected application")
            return name
        if state in {"FAILED", "DELETING"}:
            raise Refused(f"The Pre-update Backup ended {state}")
        if time.monotonic() >= give_up:
            raise Refused("The Pre-update Backup did not finish in time")
        time.sleep(bounded_timeout(min(POLL, max(give_up - time.monotonic(), 0))))
