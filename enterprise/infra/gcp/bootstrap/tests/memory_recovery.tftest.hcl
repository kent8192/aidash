mock_provider "google" {
  mock_resource "google_service_account" {
    defaults = { name = "projects/aidash-fixture/serviceAccounts/fixture@aidash-fixture.iam.gserviceaccount.com", email = "fixture@aidash-fixture.iam.gserviceaccount.com" }
  }
}
variables {
  project_id                            = "aidash-fixture"
  state_bucket_name                     = "aidash-fixture-state"
  release_bucket_name                   = "aidash-fixture-releases"
  notification_bucket_name              = "aidash-fixture-notifications"
  memory_recovery_nonproduction_enabled = true
}
run "storage_foundation_does_not_grant_history_deletion_or_project_admin" {
  command = plan
  assert {
    condition = (
      toset(google_project_iam_custom_role.memory_history[0].permissions) == toset(["storage.objects.create", "storage.objects.get", "storage.objects.list"]) &&
      toset(google_project_iam_custom_role.memory_prune[0].permissions) == toset(["storage.objects.get", "storage.objects.list", "storage.objects.delete"]) &&
      !contains(google_project_iam_custom_role.memory_bucket_create[0].permissions, "storage.buckets.delete") &&
      !contains(google_project_iam_custom_role.memory_bucket_configure[0].permissions, "storage.buckets.delete") &&
      strcontains(google_project_iam_member.memory_bucket_configure[0].condition[0].expression, "projects/_/buckets/aidash-fixture-aidash-") &&
      strcontains(google_project_iam_member.memory_bucket_configure[0].condition[0].expression, "-memory-history") &&
      strcontains(google_project_iam_member.memory_bucket_configure[0].condition[0].expression, "-memory-archives")
    )
    error_message = "Writers cannot erase safety history and automation cannot administer unrelated buckets or project IAM."
  }
}
run "production_recovery_alerts_survive_job_restarts" {
  command   = apply
  state_key = "recovery-alerts"
  variables {
    deployment_tier               = "production"
    nonproduction_project_id      = "aidash-nonproduction-fixture"
    nonproduction_state_bucket    = "aidash-nonproduction-state"
    staging_release_bucket        = "aidash-nonproduction-releases"
    production_monitoring_enabled = true
  }
  assert {
    condition = (
      strcontains(google_monitoring_alert_policy.memory_closed[0].conditions[0].condition_matched_log[0].filter, "memory_recovery_closed") &&
      strcontains(google_logging_metric.memory_prune[0].filter, "memory_recovery_prune_succeeded") &&
      strcontains(google_monitoring_alert_policy.memory_prune[0].conditions[0].condition_prometheus_query_language[0].query, "[30m]") &&
      strcontains(google_monitoring_alert_policy.memory_prune[0].conditions[0].condition_prometheus_query_language[0].query, "or vector(0)") &&
      strcontains(google_monitoring_alert_policy.memory_archives[0].conditions[0].condition_matched_log[0].filter, "jsonPayload.stale_count>0") &&
      toset(google_monitoring_alert_policy.memory_archives[0].notification_channels) == toset([google_monitoring_notification_channel.production[0].name])
    )
    error_message = "Eight-hour stale-copy events and missing prune success for thirty minutes must notify the production destination across Job restarts."
  }
}
