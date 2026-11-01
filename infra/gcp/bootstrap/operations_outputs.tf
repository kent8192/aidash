output "operations_configuration" {
  description = "Nonsecret second-phase operational configuration, not evidence of backup completion or successful recovery."
  value = {
    production_backup_enabled          = var.production_backup_enabled
    production_monitoring_enabled      = var.production_monitoring_enabled
    gke_backup_enabled                 = var.production_backup_enabled || var.production_backup_agent_enabled
    cluster                            = var.deployment_tier == "production" ? local.production_cluster_id : null
    backup_plan                        = try(google_gke_backup_backup_plan.production[0].id, null)
    pre_update_backup_plan             = try(google_gke_backup_backup_plan.pre_update[0].id, null)
    pre_update_restore_plan            = try(google_gke_backup_restore_plan.pre_update_in_place[0].id, null)
    pre_update_backup_retain_days      = var.production_pre_update_backup_retain_days
    pre_update_backup_delete_lock_days = var.production_pre_update_backup_delete_lock_days
    backup_location                    = var.production_backup_location
    backup_retain_days                 = var.production_backup_retain_days
    backup_delete_lock_days            = var.production_backup_delete_lock_days
    backup_schedule                    = var.production_backup_schedule
    backup_schedule_paused             = var.production_backup_schedule_paused
    backup_consistency                 = "crash_consistent"
    backup_freshness_max_seconds       = 93600
    restore_test_required              = true
    native_home_ledger_required        = true
    home_ledger_rto_seconds            = 7200
    home_archive_freshness_max_seconds = 28800
    home_remote_prune_max_seconds      = 1800
    volume_utilization_threshold       = var.production_volume_utilization_threshold
    restart_threshold                  = var.production_restart_threshold
    monitoring_hostname                = var.production_monitoring_hostname
    notification_channel               = try(google_monitoring_notification_channel.production[0].name, null)
    uptime_check                       = try(google_monitoring_uptime_check_config.production[0].name, null)
    alert_policies = concat(
      google_monitoring_alert_policy.production_uptime[*].name,
      [for policy in google_monitoring_alert_policy.production_workload : policy.name],
      google_monitoring_alert_policy.production_backup_failed[*].name,
      google_monitoring_alert_policy.memory_prune[*].name,
      google_monitoring_alert_policy.memory_archives[*].name,
      google_monitoring_alert_policy.memory_closed[*].name,
    )
  }
}
output "delivery_configuration" {
  description = "Complete nonsecret GCP config for the delivery orchestrator. A null input produces no publishable config. This output never enables GitHub environment flags."
  value = var.delivery_configuration == null ? null : merge(var.delivery_configuration, {
    project_id                            = var.project_id
    deployment_tier                       = var.deployment_tier
    repository                            = var.repository
    automation_branch                     = var.automation_branch
    nonproduction_project_id              = var.nonproduction_project_id
    nonproduction_state_bucket            = var.nonproduction_state_bucket
    production_node_service_account       = try(google_service_account.production_nodes[0].email, null)
    staging_project_id                    = var.deployment_tier == "production" ? var.nonproduction_project_id : ""
    staging_release_bucket                = var.staging_release_bucket
    notification_bucket                   = google_storage_bucket.notifications.name
    notification_service_account          = google_service_account.automation["notification-reader"].email
    byok_project_id                       = var.byok_project_id
    broker_service_accounts               = { for id, account in google_service_account.broker : id => account.email }
    tenant_secret_broker_service_accounts = { for id, account in google_service_account.tenant_secret_broker : id => account.email }
    broker_signing_keys                   = { for id, key in google_kms_crypto_key.capability : id => key.id }
    tenant_secret_signing_keys            = { for id, key in google_kms_crypto_key.tenant_secret : id => key.id }
    state_bucket                          = google_storage_bucket.state.name
    release_bucket                        = google_storage_bucket.releases.name
    registry                              = "us-central1-docker.pkg.dev/${var.project_id}/aidash"
    workload_identity_provider            = google_iam_workload_identity_pool_provider.github.name
    deploy_service_account                = google_service_account.automation["deploy"].email
    publish_service_account               = google_service_account.automation["publish"].email
    acme_state_secret                     = var.deployment_tier == "nonproduction" ? var.preview_acme_state_secret_id : ""
    gke_backup_enabled                    = var.production_backup_enabled || var.production_backup_agent_enabled
    pre_update_backup_plan                = try(google_gke_backup_backup_plan.pre_update[0].id, null)
    node_count                            = var.deployment_tier == "production" ? tomap({ production = 1 }) : tomap({ develop = 1, test = 1, pr = 1 })
  })
}
