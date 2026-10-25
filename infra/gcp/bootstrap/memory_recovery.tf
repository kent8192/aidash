// Bootstrap owns role definitions. Environment automation can provision only
// its named buckets and bucket bindings, without project IAM or role administration.
variable "memory_recovery_nonproduction_enabled" {
  description = "Explicitly provision the storage permission foundation for protected nonproduction Homes. Production always requires it."
  type        = bool
  default     = false
}
locals {
  memory_recovery_enabled = var.deployment_tier == "production" || var.memory_recovery_nonproduction_enabled
}
resource "google_project_iam_custom_role" "memory_history" {
  count       = local.memory_recovery_enabled ? 1 : 0
  project     = var.project_id
  role_id     = "aidashMemoryHistory"
  title       = "Aidash immutable recovery objects"
  permissions = ["storage.objects.create", "storage.objects.get", "storage.objects.list"]
}
resource "google_project_iam_custom_role" "memory_prune" {
  count       = local.memory_recovery_enabled ? 1 : 0
  project     = var.project_id
  role_id     = "aidashMemoryPrune"
  title       = "Aidash expiring recovery bodies"
  permissions = ["storage.objects.get", "storage.objects.list", "storage.objects.delete"]
}
resource "google_project_iam_custom_role" "memory_bucket_create" {
  count       = local.memory_recovery_enabled ? 1 : 0
  project     = var.project_id
  role_id     = "aidashMemoryBucketCreate"
  title       = "Aidash recovery bucket provisioning"
  permissions = ["storage.buckets.create", "storage.buckets.get", "storage.buckets.list"]
}
resource "google_project_iam_custom_role" "memory_bucket_configure" {
  count       = local.memory_recovery_enabled ? 1 : 0
  project     = var.project_id
  role_id     = "aidashMemoryBucketConfigure"
  title       = "Aidash recovery bucket configuration"
  permissions = ["storage.buckets.get", "storage.buckets.update", "storage.buckets.getIamPolicy", "storage.buckets.setIamPolicy"]
}
resource "google_project_iam_member" "memory_bucket_create" {
  count   = local.memory_recovery_enabled ? 1 : 0
  project = var.project_id
  role    = google_project_iam_custom_role.memory_bucket_create[0].name
  member  = "serviceAccount:${google_service_account.automation["deploy"].email}"
}
resource "google_project_iam_member" "memory_bucket_configure" {
  count   = local.memory_recovery_enabled ? 1 : 0
  project = var.project_id
  role    = google_project_iam_custom_role.memory_bucket_configure[0].name
  member  = "serviceAccount:${google_service_account.automation["deploy"].email}"
  condition {
    title      = "only-aidash-recovery-buckets"
    expression = "resource.name.startsWith('projects/_/buckets/${substr(var.project_id, 0, 20)}-aidash-') && (resource.name.endsWith('-memory-history') || resource.name.endsWith('-memory-archives'))"
  }
}

// Jobs emit structured receipts even when no process-wide tracing subscriber
// is installed. Monitoring retains the observations beyond each Job's lifetime.
locals {
  memory_recovery_logs = "resource.type=\"k8s_container\" AND resource.labels.cluster_name=\"${var.production_cluster_name}\" AND resource.labels.namespace_name=\"aidash-production\""
}
resource "google_logging_metric" "memory_prune" {
  count   = var.production_monitoring_enabled ? 1 : 0
  project = var.project_id
  name    = "memory_recovery_prune_success"
  filter  = "${local.memory_recovery_logs} AND resource.labels.container_name=\"memory-prune\" AND jsonPayload.event=\"memory_recovery_prune_succeeded\""
  metric_descriptor {
    metric_kind = "DELTA"
    value_type  = "INT64"
    unit        = "1"
  }
}
resource "google_monitoring_alert_policy" "memory_prune" {
  count                 = var.production_monitoring_enabled ? 1 : 0
  project               = var.project_id
  display_name          = "Aidash production remote memory pruning overdue"
  combiner              = "OR"
  enabled               = true
  notification_channels = [google_monitoring_notification_channel.production[0].name]
  conditions {
    display_name = "No successful remote prune in thirty minutes"
    condition_prometheus_query_language {
      // Sum across disposable Pods. The explicit zero also detects a Job that
      // never starts; a terminated Pod cannot erase the failure clock.
      query               = "(sum(sum_over_time(logging_googleapis_com:user_memory_recovery_prune_success{monitored_resource=\"k8s_container\"}[30m])) or vector(0)) < 1"
      duration            = "0s"
      evaluation_interval = "60s"
    }
  }
  alert_strategy { auto_close = "604800s" }
  depends_on = [google_logging_metric.memory_prune]
}
resource "google_monitoring_alert_policy" "memory_archives" {
  count                 = var.production_monitoring_enabled ? 1 : 0
  project               = var.project_id
  display_name          = "Aidash production memory archive requires attention"
  combiner              = "OR"
  enabled               = true
  notification_channels = [google_monitoring_notification_channel.production[0].name]
  conditions {
    display_name = "Archive failure, exhausted spool, or verified copy older than eight hours"
    condition_matched_log {
      filter = "${local.memory_recovery_logs} AND (jsonPayload.event=\"memory_recovery_archive_failed\" OR (jsonPayload.event=\"memory_recovery_command_failed\" AND jsonPayload.action=\"backup-all\") OR jsonPayload.stale_count>0)"
    }
  }
  alert_strategy {
    notification_rate_limit { period = "900s" }
    auto_close = "604800s"
  }
  depends_on = [google_project_service.production_operations]
}
resource "google_monitoring_alert_policy" "memory_closed" {
  count                 = var.production_monitoring_enabled ? 1 : 0
  project               = var.project_id
  display_name          = "Aidash production Home memory protection unresolved"
  combiner              = "OR"
  enabled               = true
  notification_channels = [google_monitoring_notification_channel.production[0].name]
  conditions {
    display_name = "Protected startup or floor publication requires reconciliation"
    condition_matched_log {
      // Pruning deliberately has no ledger. Only serving and migration containers
      // are safety writers whose closed state requires immediate attention.
      filter = "${local.memory_recovery_logs} AND resource.labels.container_name=\"aidash\" AND jsonPayload.event=\"memory_recovery_closed\""
    }
  }
  alert_strategy {
    notification_rate_limit { period = "300s" }
    auto_close = "604800s"
  }
  depends_on = [google_project_service.production_operations]
}
