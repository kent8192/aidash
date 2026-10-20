#!/usr/bin/env python3
"""Read deployed operational metadata without creating backups, restores or resources.

Exit success means the configured foundation and a fresh successful backup were
observed. It never proves PostgreSQL/Home recovery or enables production CD.
"""

import argparse
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import urllib.error
import urllib.parse
import urllib.request


class Refused(RuntimeError):
    pass


def timestamp(value):
    if not isinstance(value, str):
        raise Refused("A backup timestamp is missing")
    try:
        result = datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError as error:
        raise Refused("A backup timestamp is invalid") from error
    if result.tzinfo is None:
        raise Refused("Backup timestamps must include their timezone")
    return result


APPLICATION = [{"namespace": "aidash-production", "name": "aidash"}]


def application_scope(scope):
    return scope.get("selectedApplications", {}).get("namespacedNames") == APPLICATION and "selectedNamespaces" not in scope


def inspect_pre_update(operations, expected_cluster, plan, restore):
    """The Pre-update Backup plan and its in-place restore plan, which must both stay application-scoped
    so a restore never deletes the Home recovery ledger claim."""
    name = operations["pre_update_backup_plan"]
    backup_config = plan.get("backupConfig", {})
    retention = plan.get("retentionPolicy", {})
    if (plan.get("name") != name or plan.get("cluster") != expected_cluster or not application_scope(backup_config)
            or not backup_config.get("includeVolumeData") or not backup_config.get("includeSecrets")
            or backup_config.get("permissiveMode", False) or plan.get("backupSchedule", {}).get("cronSchedule")
            or retention.get("backupRetainDays") != operations.get("pre_update_backup_retain_days")
            or retention.get("backupDeleteLockDays") != operations.get("pre_update_backup_delete_lock_days")):
        raise Refused("The Pre-update Backup plan differs from configuration")
    restore_config = restore.get("restoreConfig", {})
    if (restore.get("name") != operations.get("pre_update_restore_plan") or restore.get("backupPlan") != name
            or restore.get("cluster") != expected_cluster or not application_scope(restore_config)
            or restore_config.get("namespacedResourceRestoreMode") != "DELETE_AND_RESTORE"
            or restore_config.get("volumeDataRestorePolicy") != "RESTORE_VOLUME_DATA_FROM_BACKUP"
            or "allNamespaces" in restore_config or "noNamespaces" in restore_config):
        raise Refused("The in-place restore plan could delete more than the protected application")


def inspect(config, operations, cluster, plan, backups, channel, uptime, policies, now, pre_update=None, pre_update_restore=None):
    """Check actual API observations against the nonsecret configured contract."""
    if config.get("deployment_tier") != "production":
        raise Refused("Only the isolated production profile can be inspected")
    project = config["project_id"]
    expected_cluster = f"projects/{project}/locations/us-central1/clusters/aidash"
    if operations.get("cluster") != expected_cluster:
        raise Refused("The production cluster identity differs from configuration")
    if not operations.get("production_backup_enabled") or not operations.get("production_monitoring_enabled"):
        raise Refused("Backups and monitoring have not been configured")
    if cluster.get("name") != "aidash" or cluster.get("location") != "us-central1":
        raise Refused("The observed cluster is not the configured production cluster")
    if not cluster.get("addonsConfig", {}).get("gkeBackupAgentConfig", {}).get("enabled"):
        raise Refused("The production backup agent is not enabled")
    if plan.get("name") != operations.get("backup_plan") or plan.get("cluster") != expected_cluster:
        raise Refused("The observed backup plan targets a different cluster")
    backup_config = plan.get("backupConfig", {})
    if (not application_scope(backup_config)
            or not backup_config.get("includeVolumeData") or not backup_config.get("includeSecrets")
            or backup_config.get("permissiveMode", False)):
        raise Refused("The production backup scope or retained volume data is incorrect")
    retention = plan.get("retentionPolicy", {})
    if (retention.get("backupRetainDays") != operations.get("backup_retain_days")
            or retention.get("backupDeleteLockDays") != operations.get("backup_delete_lock_days")):
        raise Refused("Backup retention differs from configuration")
    schedule = plan.get("backupSchedule", {})
    if schedule.get("paused", False) or schedule.get("cronSchedule") != operations.get("backup_schedule"):
        raise Refused("The configured daily backup schedule is missing or paused")
    successes = [backup for backup in backups if backup.get("state") == "SUCCEEDED"]
    if not successes:
        raise Refused("No successful production backup has been observed")
    latest = max(successes, key=lambda backup: timestamp(backup.get("createTime")))
    created = timestamp(latest.get("createTime"))
    completed = timestamp(latest.get("completeTime"))
    age = (now - created).total_seconds()
    maximum_age = operations.get("backup_freshness_max_seconds")
    if not isinstance(maximum_age, int) or not 86400 <= maximum_age <= 172800:
        raise Refused("An explicit daily backup freshness limit is required")
    if created > completed or completed > now or not 0 <= age <= maximum_age:
        raise Refused("The latest successful backup is stale or its timestamps are invalid")
    if (not latest.get("name", "").startswith(plan["name"] + "/backups/")
            or latest.get("clusterMetadata", {}).get("cluster") != expected_cluster
            or not application_scope(latest)
            or not latest.get("containsVolumeData") or not latest.get("containsSecrets")
            or latest.get("volumeCount", 0) < 1):
        raise Refused("The successful backup does not contain the configured production data")
    configured = bool(operations.get("pre_update_backup_plan"))
    if configured:
        if pre_update is None or pre_update_restore is None:
            raise Refused("The Pre-update Backup plan and restore plan were not observed")
        inspect_pre_update(operations, expected_cluster, pre_update, pre_update_restore)
    expected_channel = operations.get("notification_channel")
    if (channel.get("name") != expected_channel or channel.get("type") != "email"
            or channel.get("labels", {}).get("email_address") != "prod.deployment@aidash.run"
            or not channel.get("enabled") or channel.get("verificationStatus") == "UNVERIFIED"):
        raise Refused("The fixed production notification channel is not usable")
    http = uptime.get("httpCheck", {})
    if (uptime.get("name") != operations.get("uptime_check")
            or uptime.get("monitoredResource", {}).get("labels", {}).get("host") != operations.get("monitoring_hostname")
            or uptime.get("monitoredResource", {}).get("labels", {}).get("project_id") != project
            or not http.get("useSsl") or not http.get("validateSsl") or http.get("path") != "/health"
            or http.get("port") != 443
            or http.get("acceptedResponseStatusCodes") != [{"statusValue": 200}]
            or uptime.get("contentMatchers") != [{"content": "aidash://aidash-production-", "matcher": "CONTAINS_STRING"}]):
        raise Refused("The observed uptime check does not validate the production HTTPS origin")
    expected_policies = operations.get("alert_policies", [])
    if len(expected_policies) != 4 or {p.get("name") for p in policies} != set(expected_policies):
        raise Refused("The configured production alert policies are missing")
    if any(not policy.get("enabled") or policy.get("notificationChannels") != [expected_channel] for policy in policies):
        raise Refused("A production alert policy is disabled or targets another channel")
    by_name = {policy.get("displayName"): policy for policy in policies}
    expected = {
        "Aidash production HTTPS unavailable": ("monitoring.googleapis.com/uptime_check/check_passed", "COMPARISON_LT", 1, "300s", "ALIGN_FRACTION_TRUE", 2),
        "Aidash production retained volume capacity": ("kubernetes.io/pod/volume/utilization", "COMPARISON_GT", operations.get("volume_utilization_threshold"), "300s", "ALIGN_MAX", 1),
        "Aidash production repeated container restarts": ("kubernetes.io/container/restart_count", "COMPARISON_GT", operations.get("restart_threshold", 0) - 1, "0s", "ALIGN_DELTA", 1),
        "Aidash production backup failed": ("gkebackup.googleapis.com/backup_completion_times", "COMPARISON_GT", 0, "0s", "ALIGN_PERCENTILE_99", 1),
    }
    if set(by_name) != set(expected):
        raise Refused("The production alert policy identities differ from configuration")
    expected_filters = {
        "Aidash production HTTPS unavailable": f'resource.type = "uptime_url" AND metric.type = "monitoring.googleapis.com/uptime_check/check_passed" AND metric.labels.check_id = "{operations["uptime_check"].rsplit("/", 1)[-1]}"',
        "Aidash production retained volume capacity": 'resource.type = "k8s_pod" AND metric.type = "kubernetes.io/pod/volume/utilization" AND resource.labels.cluster_name = "aidash" AND resource.labels.namespace_name = "aidash-production" AND metric.labels.persistentvolumeclaim_namespace = "aidash-production"',
        "Aidash production repeated container restarts": 'resource.type = "k8s_container" AND metric.type = "kubernetes.io/container/restart_count" AND resource.labels.cluster_name = "aidash" AND resource.labels.namespace_name = "aidash-production"',
        "Aidash production backup failed": 'resource.type = "gkebackup.googleapis.com/BackupPlan" AND metric.type = "gkebackup.googleapis.com/backup_completion_times" AND metric.labels.state = "FAILED"',
    }
    for name, (metric, comparison, threshold, duration, aligner, trigger) in expected.items():
        conditions = by_name[name].get("conditions", [])
        condition = conditions[0].get("conditionThreshold", {}) if len(conditions) == 1 else {}
        expression = condition.get("filter", "")
        aggregations = condition.get("aggregations", [])
        if (" ".join(expression.split()) != expected_filters[name]
                or condition.get("comparison") != comparison or condition.get("thresholdValue", 0) != threshold
                or condition.get("duration", "0s") != duration or condition.get("trigger", {}).get("count") != trigger
                or len(aggregations) != 1 or aggregations[0].get("perSeriesAligner") != aligner
                or aggregations[0].get("alignmentPeriod") != ("60s" if metric.startswith("monitoring.") else "300s")
                or by_name[name].get("combiner") != "OR"):
            raise Refused("A production alert condition differs from configuration")
    return {
        "protocol": "aidash-production-operations-observation/1",
        "project_id": project,
        "cluster": expected_cluster,
        "backup": latest["name"],
        "backup_created_at": latest["createTime"],
        "backup_completed_at": latest["completeTime"],
        "backup_age_seconds": age,
        "configuration_verified": True,
        "successful_backup_observed": True,
        "pre_update_plans_verified": configured,
        "notification_delivery_proven": False,
        "restore_verified": False,
        "required_next_evidence": ["isolated_postgresql_restore", "independent_native_home_ledger", "jetstream_and_runner_journal_reconciliation", "mail_delivery", "iam_denial_checks"],
    }


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, response, code, message, headers, target):
        return None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--terraform-output", required=True, type=Path, help="Private file from terraform output -json; secrets are never printed")
    args = parser.parse_args()
    output = json.loads(args.terraform_output.read_text())
    config = output["delivery_configuration"]["value"]
    operations = output["operations_configuration"]["value"]
    if not config or config.get("deployment_tier") != "production":
        raise Refused("A complete production bootstrap configuration is required")
    project = config.get("project_id", "")
    if not re.fullmatch(r"[a-z][a-z0-9-]{4,28}[a-z0-9]", project):
        raise Refused("An explicit production project ID is required")
    if project == config.get("nonproduction_project_id") or not config.get("nonproduction_project_id"):
        raise Refused("Production must be isolated from the explicit nonproduction project")
    if operations.get("cluster") != f"projects/{project}/locations/us-central1/clusters/aidash":
        raise Refused("Only the configured existing production cluster may be read")
    if not operations.get("production_backup_enabled") or not operations.get("production_monitoring_enabled"):
        raise Refused("Production operations have not been enabled")
    token = os.environ.get("GOOGLE_OAUTH_ACCESS_TOKEN") or subprocess.check_output(
        ["gcloud", "auth", "print-access-token", "--project", project], text=True, timeout=60
    ).strip()
    opener = urllib.request.build_opener(NoRedirect())

    def get(service, resource, query=None):
        if not re.fullmatch(rf"projects/{re.escape(project)}/[a-zA-Z0-9_/-]+", resource):
            raise Refused("A returned resource is outside the configured production project")
        url = f"https://{service}.googleapis.com/" + ("v3/" if service == "monitoring" else "v1/") + resource
        if query:
            url += "?" + urllib.parse.urlencode(query)
        request = urllib.request.Request(url, headers={"Authorization": "Bearer " + token})
        with opener.open(request, timeout=30) as response:
            return json.load(response)

    cluster = get("container", operations["cluster"])
    plan = get("gkebackup", operations["backup_plan"])
    backups, page = [], None
    for _ in range(100):
        query = {"pageSize": 1000}
        if page:
            query["pageToken"] = page
        result = get("gkebackup", operations["backup_plan"] + "/backups", query)
        backups.extend(result.get("backups", []))
        page = result.get("nextPageToken")
        if not page:
            break
    else:
        raise Refused("The backup inventory did not finish; refuse a partial observation")
    pre_update = get("gkebackup", operations["pre_update_backup_plan"]) if operations.get("pre_update_backup_plan") else None
    pre_update_restore = get("gkebackup", operations["pre_update_restore_plan"]) if operations.get("pre_update_restore_plan") else None
    channel = get("monitoring", operations["notification_channel"])
    uptime = get("monitoring", operations["uptime_check"])
    policies = [get("monitoring", name) for name in operations["alert_policies"]]
    evidence = inspect(config, operations, cluster, plan, backups, channel, uptime, policies, datetime.now(timezone.utc), pre_update, pre_update_restore)
    print(json.dumps(evidence, indent=2))


if __name__ == "__main__":
    try:
        main()
    except urllib.error.HTTPError as error:
        sys.exit(f"Read-only operational inspection failed with HTTP {error.code}")
    except (Refused, KeyError, ValueError, OSError, subprocess.SubprocessError) as error:
        sys.exit(f"Operational inspection refused: {error}")
