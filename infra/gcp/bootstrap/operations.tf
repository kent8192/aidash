// This second bootstrap phase runs only after production's existing cluster is
// available. Neither backups nor alerts enable CI/CD or create another cluster.
locals {
  production_cluster_id = "projects/${var.project_id}/locations/${var.production_cluster_location}/clusters/${var.production_cluster_name}"
  production_operations_services = toset(concat(
    var.production_backup_enabled || var.production_backup_agent_enabled ? ["gkebackup.googleapis.com"] : [],
    var.production_monitoring_enabled ? ["monitoring.googleapis.com"] : [],
  ))
  production_workload_alerts = var.production_monitoring_enabled ? {
    volume_capacity = {
      display_name = "Aidash production retained volume capacity"
      filter       = "resource.type = \"k8s_pod\" AND metric.type = \"kubernetes.io/pod/volume/utilization\" AND resource.labels.cluster_name = \"${var.production_cluster_name}\" AND resource.labels.namespace_name = \"aidash-production\" AND metric.labels.persistentvolumeclaim_namespace = \"aidash-production\""
      aligner      = "ALIGN_MAX"
      threshold    = var.production_volume_utilization_threshold
      duration     = "300s"
    }
    container_restarts = {
      display_name = "Aidash production repeated container restarts"
      filter       = "resource.type = \"k8s_container\" AND metric.type = \"kubernetes.io/container/restart_count\" AND resource.labels.cluster_name = \"${var.production_cluster_name}\" AND resource.labels.namespace_name = \"aidash-production\""
      aligner      = "ALIGN_DELTA"
      threshold    = var.production_restart_threshold - 1
      duration     = "0s"
    }
  } : {}
}
resource "google_project_service" "production_operations" {
  for_each           = local.production_operations_services
  project            = var.project_id
  service            = each.value
  disable_on_destroy = false
}
data "google_container_cluster" "production_operations" {
  count    = var.production_backup_enabled || var.production_monitoring_enabled ? 1 : 0
  project  = var.project_id
  name     = var.production_cluster_name
  location = var.production_cluster_location
}
resource "google_gke_backup_backup_plan" "production" {
  count           = var.production_backup_enabled ? 1 : 0
  project         = var.project_id
  name            = "aidash-production-daily"
  location        = var.production_backup_location
  cluster         = local.production_cluster_id
  deletion_policy = "PREVENT"
  description     = "Production namespace/PVC crash-consistent backup. Configuration is not restore verification; PostgreSQL and Home ledger acceptance are separate."
  labels          = { application = "aidash", lifecycle = "production" }
  retention_policy {
    backup_retain_days      = var.production_backup_retain_days
    backup_delete_lock_days = var.production_backup_delete_lock_days
    locked                  = false
  }
  backup_schedule {
    cron_schedule = var.production_backup_schedule
    paused        = var.production_backup_schedule_paused
  }
  backup_config {
    include_volume_data = true
    include_secrets     = true
    permissive_mode     = false
    selected_namespaces { namespaces = ["aidash-production"] }
  }
  lifecycle {
    prevent_destroy = true
    precondition {
      condition     = try(data.google_container_cluster.production_operations[0].addons_config[0].gke_backup_agent_config[0].enabled, false)
      error_message = "Enable and observe the Backup for GKE agent on the existing production cluster before applying its backup plan."
    }
  }
  depends_on = [google_project_service.production_operations]
}
resource "google_monitoring_notification_channel" "production" {
  count        = var.production_monitoring_enabled ? 1 : 0
  project      = var.project_id
  display_name = "Aidash production deployment mailbox"
  type         = "email"
  labels       = { email_address = "prod.deployment@aidash.run" }
  enabled      = true
  user_labels  = { application = "aidash", lifecycle = "production" }
  depends_on   = [google_project_service.production_operations]
}
resource "google_monitoring_uptime_check_config" "production" {
  count            = var.production_monitoring_enabled ? 1 : 0
  project          = var.project_id
  display_name     = "Aidash production HTTPS health"
  timeout          = "10s"
  period           = "60s"
  selected_regions = ["USA"]
  checker_type     = "STATIC_IP_CHECKERS"
  http_check {
    path           = "/health"
    port           = 443
    use_ssl        = true
    validate_ssl   = true
    request_method = "GET"
    accepted_response_status_codes { status_value = 200 }
  }
  content_matchers {
    content = "aidash://aidash-production-"
    matcher = "CONTAINS_STRING"
  }
  monitored_resource {
    type = "uptime_url"
    labels = {
      project_id = var.project_id
      host       = var.production_monitoring_hostname
    }
  }
  user_labels = { application = "aidash", lifecycle = "production" }
  depends_on  = [google_project_service.production_operations]
}
resource "google_monitoring_alert_policy" "production_uptime" {
  count                 = var.production_monitoring_enabled ? 1 : 0
  project               = var.project_id
  display_name          = "Aidash production HTTPS unavailable"
  combiner              = "OR"
  enabled               = true
  notification_channels = [google_monitoring_notification_channel.production[0].name]
  user_labels           = { application = "aidash", lifecycle = "production" }
  conditions {
    display_name = "At least two checkers fail for five minutes"
    condition_threshold {
      filter          = "resource.type = \"uptime_url\" AND metric.type = \"monitoring.googleapis.com/uptime_check/check_passed\" AND metric.labels.check_id = \"${google_monitoring_uptime_check_config.production[0].uptime_check_id}\""
      comparison      = "COMPARISON_LT"
      threshold_value = 1
      duration        = "300s"
      aggregations {
        alignment_period   = "60s"
        per_series_aligner = "ALIGN_FRACTION_TRUE"
      }
      trigger { count = 2 }
    }
  }
  alert_strategy { auto_close = "604800s" }
}
resource "google_monitoring_alert_policy" "production_workload" {
  for_each              = local.production_workload_alerts
  project               = var.project_id
  display_name          = each.value.display_name
  combiner              = "OR"
  enabled               = true
  notification_channels = [google_monitoring_notification_channel.production[0].name]
  user_labels           = { application = "aidash", lifecycle = "production" }
  conditions {
    display_name = each.value.display_name
    condition_threshold {
      filter          = each.value.filter
      comparison      = "COMPARISON_GT"
      threshold_value = each.value.threshold
      duration        = each.value.duration
      aggregations {
        alignment_period   = "300s"
        per_series_aligner = each.value.aligner
      }
      trigger { count = 1 }
    }
  }
  alert_strategy { auto_close = "604800s" }
}
resource "google_monitoring_alert_policy" "production_backup_failed" {
  count                 = var.production_monitoring_enabled && var.production_backup_enabled ? 1 : 0
  project               = var.project_id
  display_name          = "Aidash production backup failed"
  combiner              = "OR"
  enabled               = true
  notification_channels = [google_monitoring_notification_channel.production[0].name]
  user_labels           = { application = "aidash", lifecycle = "production" }
  conditions {
    display_name = "Backup for GKE reports a failed completion"
    condition_threshold {
      filter          = "resource.type = \"gkebackup.googleapis.com/BackupPlan\" AND metric.type = \"gkebackup.googleapis.com/backup_completion_times\" AND metric.labels.state = \"FAILED\""
      comparison      = "COMPARISON_GT"
      threshold_value = 0
      duration        = "0s"
      aggregations {
        alignment_period   = "300s"
        per_series_aligner = "ALIGN_PERCENTILE_99"
      }
      trigger { count = 1 }
    }
  }
  alert_strategy { auto_close = "604800s" }
}
