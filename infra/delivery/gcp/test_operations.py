"""Contracts for read-only production API observations, independent of IaC success."""

from copy import deepcopy
from datetime import datetime, timezone
import unittest

from inspect_operations import inspect, Refused


APPLICATION = {"namespace": "aidash-production", "name": "aidash"}


class OperationalObservationTests(unittest.TestCase):
    def setUp(self):
        project = "aidash-production"
        cluster = f"projects/{project}/locations/us-central1/clusters/aidash"
        plan = f"projects/{project}/locations/us-central1/backupPlans/aidash-production-daily"
        channel = f"projects/{project}/notificationChannels/123"
        uptime = f"projects/{project}/uptimeCheckConfigs/check-123"
        self.config = {"project_id": project, "deployment_tier": "production"}
        self.operations = {
            "cluster": cluster, "production_backup_enabled": True, "production_monitoring_enabled": True,
            "backup_plan": plan, "backup_retain_days": 30, "backup_delete_lock_days": 7,
            "backup_schedule": "0 19 * * *", "backup_freshness_max_seconds": 93600,
            "monitoring_hostname": "aidash.run", "notification_channel": channel, "uptime_check": uptime,
            "volume_utilization_threshold": 0.8, "restart_threshold": 5,
            "alert_policies": [f"projects/{project}/alertPolicies/{number}" for number in range(4)],
        }
        self.cluster = {"name": "aidash", "location": "us-central1", "addonsConfig": {"gkeBackupAgentConfig": {"enabled": True}}}
        self.plan = {
            "name": plan, "cluster": cluster,
            "backupConfig": {"selectedApplications": {"namespacedNames": [APPLICATION]}, "includeVolumeData": True, "includeSecrets": True},
            "retentionPolicy": {"backupRetainDays": 30, "backupDeleteLockDays": 7},
            "backupSchedule": {"cronSchedule": "0 19 * * *"},
        }
        self.backup = {
            "name": plan + "/backups/observed", "state": "SUCCEEDED", "createTime": "2026-10-12T19:00:00Z", "completeTime": "2026-10-12T19:15:00Z",
            "clusterMetadata": {"cluster": cluster}, "selectedApplications": {"namespacedNames": [APPLICATION]},
            "containsVolumeData": True, "containsSecrets": True, "volumeCount": 5,
        }
        self.channel = {"name": channel, "type": "email", "labels": {"email_address": "prod.deployment@aidash.run"}, "enabled": True}
        self.uptime = {
            "name": uptime, "monitoredResource": {"labels": {"host": "aidash.run", "project_id": project}},
            "httpCheck": {"useSsl": True, "validateSsl": True, "port": 443, "path": "/health", "acceptedResponseStatusCodes": [{"statusValue": 200}]},
            "contentMatchers": [{"content": "aidash://aidash-production-", "matcher": "CONTAINS_STRING"}],
        }
        rows = [
            ("Aidash production HTTPS unavailable", 'resource.type = "uptime_url" AND metric.type = "monitoring.googleapis.com/uptime_check/check_passed" AND metric.labels.check_id = "check-123"', "COMPARISON_LT", 1, "300s", "ALIGN_FRACTION_TRUE", 2, "60s"),
            ("Aidash production retained volume capacity", 'resource.type = "k8s_pod" AND metric.type = "kubernetes.io/pod/volume/utilization" AND resource.labels.cluster_name = "aidash" AND resource.labels.namespace_name = "aidash-production" AND metric.labels.persistentvolumeclaim_namespace = "aidash-production"', "COMPARISON_GT", 0.8, "300s", "ALIGN_MAX", 1, "300s"),
            ("Aidash production repeated container restarts", 'resource.type = "k8s_container" AND metric.type = "kubernetes.io/container/restart_count" AND resource.labels.cluster_name = "aidash" AND resource.labels.namespace_name = "aidash-production"', "COMPARISON_GT", 4, "0s", "ALIGN_DELTA", 1, "300s"),
            ("Aidash production backup failed", 'resource.type = "gkebackup.googleapis.com/BackupPlan" AND metric.type = "gkebackup.googleapis.com/backup_completion_times" AND metric.labels.state = "FAILED"', "COMPARISON_GT", 0, "0s", "ALIGN_PERCENTILE_99", 1, "300s"),
        ]
        self.policies = [
            {"name": self.operations["alert_policies"][index], "displayName": name, "enabled": True, "combiner": "OR", "notificationChannels": [channel],
             "conditions": [{"conditionThreshold": {"filter": metric_filter, "comparison": comparison, "thresholdValue": threshold, "duration": duration,
                             "aggregations": [{"perSeriesAligner": aligner, "alignmentPeriod": period}], "trigger": {"count": trigger}}}]}
            for index, (name, metric_filter, comparison, threshold, duration, aligner, trigger, period) in enumerate(rows)
        ]
        self.now = datetime(2026, 10, 13, 0, 0, tzinfo=timezone.utc)
        pre_update = f"projects/{project}/locations/us-central1/backupPlans/aidash-production-pre-update"
        restore = f"projects/{project}/locations/us-central1/restorePlans/aidash-production-pre-update-in-place"
        self.operations.update(
            pre_update_backup_plan=pre_update, pre_update_restore_plan=restore,
            pre_update_backup_retain_days=14, pre_update_backup_delete_lock_days=2,
        )
        self.pre_update = {
            "name": pre_update, "cluster": cluster,
            "backupConfig": {"selectedApplications": {"namespacedNames": [APPLICATION]}, "includeVolumeData": True, "includeSecrets": True},
            "retentionPolicy": {"backupRetainDays": 14, "backupDeleteLockDays": 2},
        }
        self.restore = {
            "name": restore, "backupPlan": pre_update, "cluster": cluster,
            "restoreConfig": {
                "selectedApplications": {"namespacedNames": [APPLICATION]},
                "namespacedResourceRestoreMode": "DELETE_AND_RESTORE", "volumeDataRestorePolicy": "RESTORE_VOLUME_DATA_FROM_BACKUP",
                "clusterResourceRestoreScope": {"noGroupKinds": True},
            },
        }

    def observe(self, backups=None):
        return inspect(
            self.config, self.operations, self.cluster, self.plan, backups if backups is not None else [self.backup],
            self.channel, self.uptime, self.policies, self.now, self.pre_update, self.restore,
        )

    def test_successful_snapshot_and_config_never_prove_restore_or_mail_delivery(self):
        observation = self.observe()
        self.assertTrue(observation["successful_backup_observed"])
        self.assertFalse(observation["restore_verified"])
        self.assertFalse(observation["notification_delivery_proven"])
        self.assertIn("independent_native_home_ledger", observation["required_next_evidence"])

    def test_pre_update_plans_are_reported_and_may_be_absent_from_older_outputs(self):
        self.assertTrue(self.observe()["pre_update_plans_verified"])
        for key in ("pre_update_backup_plan", "pre_update_restore_plan"):
            del self.operations[key]
        self.pre_update = self.restore = None
        self.assertFalse(self.observe()["pre_update_plans_verified"])

    def test_the_daily_plan_and_backup_must_select_exactly_the_protected_application(self):
        for mutate in (
            lambda: self.plan["backupConfig"].update(selectedNamespaces={"namespaces": ["aidash-production"]}),
            lambda: self.plan["backupConfig"].update(selectedApplications={"namespacedNames": [APPLICATION, {"namespace": "other", "name": "x"}]}),
            lambda: self.backup.pop("selectedApplications"),
        ):
            self.setUp()
            mutate()
            with self.assertRaises(Refused):
                self.observe()

    def test_pre_update_plans_must_stay_unscheduled_application_scoped_and_unable_to_delete_the_namespace(self):
        mutations = [
            (lambda: self.pre_update.update(backupSchedule={"cronSchedule": "0 19 * * *"}), "Pre-update Backup plan"),
            (lambda: self.pre_update["backupConfig"].update(selectedNamespaces={"namespaces": ["aidash-production"]}), "Pre-update Backup plan"),
            (lambda: self.pre_update["retentionPolicy"].update(backupRetainDays=30), "Pre-update Backup plan"),
            (lambda: self.restore["restoreConfig"].pop("selectedApplications"), "could delete more"),
            (lambda: self.restore["restoreConfig"].update(selectedNamespaces={"namespaces": ["aidash-production"]}), "could delete more"),
            (lambda: self.restore["restoreConfig"].update(allNamespaces=True), "could delete more"),
            (lambda: self.restore.update(backupPlan=self.plan["name"]), "could delete more"),
        ]
        for mutate, message in mutations:
            with self.subTest(message=message):
                self.setUp()
                mutate()
                with self.assertRaisesRegex(Refused, message):
                    self.observe()
        self.setUp()
        self.pre_update = None
        with self.assertRaisesRegex(Refused, "not observed"):
            self.observe()

    def test_the_restore_plan_must_leave_cluster_resources_untouched(self):
        for scope in (
            None,
            {"noGroupKinds": False},
            {"allGroupKinds": True},
            {"noGroupKinds": True, "selectedGroupKinds": [{"group": "rbac.authorization.k8s.io", "kind": "ClusterRole"}]},
            {"noGroupKinds": True, "excludedGroupKinds": [{"group": "", "kind": "Namespace"}]},
        ):
            with self.subTest(scope=scope):
                self.setUp()
                if scope is None:
                    self.restore["restoreConfig"].pop("clusterResourceRestoreScope")
                else:
                    self.restore["restoreConfig"]["clusterResourceRestoreScope"] = scope
                with self.assertRaisesRegex(Refused, "could delete more"):
                    self.observe()

    def test_recent_completion_cannot_hide_stale_snapshot_or_missing_actual_backup(self):
        stale = deepcopy(self.backup)
        stale["createTime"] = "2026-10-10T19:00:00Z"
        with self.assertRaisesRegex(Refused, "stale"):
            self.observe([stale])
        with self.assertRaisesRegex(Refused, "No successful"):
            self.observe([])

    def test_observed_scope_tls_recipient_and_alert_drift_are_rejected(self):
        mutations = [
            (lambda: self.backup.update(containsVolumeData=False), "configured production data"),
            (lambda: self.channel["labels"].update(email_address="other@example.com"), "notification channel"),
            (lambda: self.uptime["httpCheck"].update(validateSsl=False), "HTTPS origin"),
            (lambda: self.policies[2]["conditions"][0]["conditionThreshold"].update(filter='metric.type = "kubernetes.io/container/restart_count"'), "alert condition"),
        ]
        for mutate, message in mutations:
            with self.subTest(message=message):
                self.setUp()
                mutate()
                with self.assertRaisesRegex(Refused, message):
                    self.observe()


if __name__ == "__main__":
    unittest.main()
