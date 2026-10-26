mock_provider "google" {
  mock_resource "google_service_account" {
    defaults = {
      name  = "projects/aidash-fixture/serviceAccounts/fixture@aidash-fixture.iam.gserviceaccount.com"
      email = "fixture@aidash-fixture.iam.gserviceaccount.com"
    }
  }
  mock_resource "google_monitoring_notification_channel" {
    defaults = { name = "projects/aidash-fixture/notificationChannels/123" }
  }
  mock_resource "google_monitoring_uptime_check_config" {
    defaults = { uptime_check_id = "aidash-production-health-123" }
  }
  mock_data "google_container_cluster" {
    defaults = {
      addons_config = [{ gke_backup_agent_config = [{ enabled = true }] }]
    }
  }
}
variables {
  project_id               = "aidash-fixture"
  state_bucket_name        = "aidash-fixture-state"
  release_bucket_name      = "aidash-fixture-releases"
  notification_bucket_name = "aidash-fixture-notifications"
}
run "initial_bootstrap_does_not_start_operational_services" {
  command = plan
  assert {
    condition = (
      length(google_project_service.production_operations) == 0 &&
      length(google_gke_backup_backup_plan.production) == 0 &&
      length(google_gke_backup_backup_plan.pre_update) == 0 &&
      length(google_gke_backup_restore_plan.pre_update_in_place) == 0 &&
      length(google_monitoring_uptime_check_config.production) == 0 &&
      length(google_monitoring_alert_policy.production_workload) == 0 &&
      output.delivery_configuration == null &&
      !output.operations_configuration.production_backup_enabled &&
      !output.operations_configuration.production_monitoring_enabled
    )
    error_message = "Initial bootstrap must not depend on a cluster, create backups/alerts, or produce a deployment config by default."
  }
}
run "complete_nonproduction_configuration" {
  command   = apply
  state_key = "nonproduction-configuration"
  variables {
    preview_tls_secret_id = "aidash-preview-wildcard-tls"
    delivery_configuration = {
      cloudflare_zone_id    = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
      release_app_bot_login = "aidash-release[bot]"
      preview_tls_secret    = "aidash-preview-wildcard-tls"
    }
  }
  assert {
    condition = (
      output.delivery_configuration.deployment_tier == "nonproduction" &&
      output.delivery_configuration.state_bucket == "aidash-fixture-state" &&
      output.delivery_configuration.notification_bucket == "aidash-fixture-notifications" &&
      output.delivery_configuration.automation_branch == "develop/0.1.0" &&
      output.delivery_configuration.trusted_branch == "main" &&
      output.delivery_configuration.preview_tls_secret == "aidash-preview-wildcard-tls" &&
      !output.delivery_configuration.release_flow_enabled &&
      !output.delivery_configuration.gke_backup_enabled &&
      output.delivery_configuration.node_count == tomap({ develop = 1, test = 1, pr = 1 })
    )
    error_message = "Generated nonproduction configuration must include exact identities, buckets and trusted flow inputs without enabling delivery."
  }
}
run "production_backup_and_monitoring_are_scoped" {
  command   = apply
  state_key = "production-operations"
  variables {
    deployment_tier               = "production"
    nonproduction_project_id      = "aidash-nonproduction-fixture"
    nonproduction_state_bucket    = "aidash-nonproduction-state"
    staging_release_bucket        = "aidash-nonproduction-releases"
    production_backup_enabled     = true
    production_monitoring_enabled = true
    delivery_configuration = {
      cloudflare_zone_id    = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
      release_app_bot_login = "aidash-release[bot]"
    }
  }
  assert {
    condition = (
      google_gke_backup_backup_plan.production[0].cluster == "projects/aidash-fixture/locations/us-central1/clusters/aidash" &&
      length(google_gke_backup_backup_plan.production[0].backup_config[0].selected_namespaces) == 0 &&
      google_gke_backup_backup_plan.production[0].backup_config[0].selected_applications[0].namespaced_names == tolist([{ namespace = "aidash-production", name = "aidash" }]) &&
      google_gke_backup_backup_plan.production[0].backup_config[0].include_volume_data &&
      google_gke_backup_backup_plan.production[0].backup_config[0].include_secrets &&
      !google_gke_backup_backup_plan.production[0].backup_config[0].permissive_mode &&
      google_gke_backup_backup_plan.production[0].retention_policy[0].backup_retain_days == 30 &&
      google_gke_backup_backup_plan.production[0].retention_policy[0].backup_delete_lock_days == 7 &&
      !google_gke_backup_backup_plan.production[0].retention_policy[0].locked &&
      google_gke_backup_backup_plan.production[0].deletion_policy == "PREVENT"
    )
    error_message = "Only the production application (not the whole namespace) is backed up with explicit retention and deletion protection."
  }
  assert {
    condition = (
      google_gke_backup_backup_plan.pre_update[0].cluster == "projects/aidash-fixture/locations/us-central1/clusters/aidash" &&
      length(google_gke_backup_backup_plan.pre_update[0].backup_schedule) == 0 &&
      length(google_gke_backup_backup_plan.pre_update[0].backup_config[0].selected_namespaces) == 0 &&
      google_gke_backup_backup_plan.pre_update[0].backup_config[0].selected_applications[0].namespaced_names == tolist([{ namespace = "aidash-production", name = "aidash" }]) &&
      google_gke_backup_backup_plan.pre_update[0].backup_config[0].include_volume_data &&
      google_gke_backup_backup_plan.pre_update[0].backup_config[0].include_secrets &&
      !google_gke_backup_backup_plan.pre_update[0].backup_config[0].permissive_mode &&
      google_gke_backup_backup_plan.pre_update[0].retention_policy[0].backup_retain_days == 14 &&
      google_gke_backup_backup_plan.pre_update[0].retention_policy[0].backup_delete_lock_days == 2 &&
      google_gke_backup_backup_plan.pre_update[0].deletion_policy == "PREVENT"
    )
    error_message = "The Pre-update Backup plan is unscheduled, scoped to the application, and keeps backups 14 days with a 2 day lock."
  }
  assert {
    condition = (
      google_gke_backup_restore_plan.pre_update_in_place[0].backup_plan == google_gke_backup_backup_plan.pre_update[0].id &&
      google_gke_backup_restore_plan.pre_update_in_place[0].cluster == "projects/aidash-fixture/locations/us-central1/clusters/aidash" &&
      google_gke_backup_restore_plan.pre_update_in_place[0].restore_config[0].selected_applications[0].namespaced_names == tolist([{ namespace = "aidash-production", name = "aidash" }]) &&
      length(google_gke_backup_restore_plan.pre_update_in_place[0].restore_config[0].selected_namespaces) == 0 &&
      google_gke_backup_restore_plan.pre_update_in_place[0].restore_config[0].namespaced_resource_restore_mode == "DELETE_AND_RESTORE" &&
      google_gke_backup_restore_plan.pre_update_in_place[0].restore_config[0].volume_data_restore_policy == "RESTORE_VOLUME_DATA_FROM_BACKUP" &&
      google_gke_backup_restore_plan.pre_update_in_place[0].restore_config[0].cluster_resource_restore_scope[0].no_group_kinds &&
      google_gke_backup_restore_plan.pre_update_in_place[0].deletion_policy == "PREVENT"
    )
    error_message = "The in-place restore plan must delete and restore only the application, never the namespace that holds the Home recovery ledger."
  }
  assert {
    condition = (
      google_gke_backup_backup_plan_iam_member.pre_update_deploy[0].name == google_gke_backup_backup_plan.pre_update[0].name &&
      google_gke_backup_backup_plan_iam_member.pre_update_deploy[0].role == "roles/gkebackup.delegatedBackupAdmin" &&
      google_gke_backup_backup_plan_iam_member.pre_update_deploy[0].member == "serviceAccount:${google_service_account.automation["deploy"].email}" &&
      output.operations_configuration.pre_update_backup_plan == google_gke_backup_backup_plan.pre_update[0].id &&
      output.operations_configuration.pre_update_restore_plan == google_gke_backup_restore_plan.pre_update_in_place[0].id &&
      output.operations_configuration.pre_update_backup_retain_days == 14 &&
      output.operations_configuration.pre_update_backup_delete_lock_days == 2 &&
      output.delivery_configuration.pre_update_backup_plan == google_gke_backup_backup_plan.pre_update[0].id
    )
    error_message = "The deploy identity may only create Pre-update Backups on their own plan, and both plans are published."
  }
  assert {
    condition = (
      google_monitoring_uptime_check_config.production[0].http_check[0].use_ssl &&
      google_monitoring_uptime_check_config.production[0].http_check[0].validate_ssl &&
      google_monitoring_uptime_check_config.production[0].http_check[0].path == "/health" &&
      google_monitoring_uptime_check_config.production[0].content_matchers[0].content == "aidash://aidash-production-" &&
      google_monitoring_notification_channel.production[0].labels.email_address == "prod.deployment@aidash.run" &&
      google_monitoring_alert_policy.production_uptime[0].conditions[0].condition_threshold[0].trigger[0].count == 2 &&
      length(output.operations_configuration.alert_policies) == 7 &&
      output.operations_configuration.restore_test_required &&
      output.operations_configuration.native_home_ledger_required &&
      !output.delivery_configuration.release_flow_enabled &&
      output.delivery_configuration.node_count == tomap({ production = 1 })
    )
    error_message = "Alerts require valid production HTTPS, use the fixed mailbox, and never substitute backup configuration for restore proof or deployment authorization."
  }
  assert {
    condition = (
      google_monitoring_alert_policy.production_workload["container_restarts"].conditions[0].condition_threshold[0].aggregations[0].per_series_aligner == "ALIGN_DELTA" &&
      strcontains(google_monitoring_alert_policy.production_workload["volume_capacity"].conditions[0].condition_threshold[0].filter, "aidash-production") &&
      strcontains(google_monitoring_alert_policy.production_backup_failed[0].conditions[0].condition_threshold[0].filter, "metric.labels.state = \"FAILED\"")
    )
    error_message = "Capacity/restart/backup failure policies must use the real metric kinds and only production resources."
  }
}
run "restore_plan_stays_in_the_target_cluster_region" {
  command = plan
  variables {
    deployment_tier            = "production"
    nonproduction_project_id   = "aidash-nonproduction-fixture"
    nonproduction_state_bucket = "aidash-nonproduction-state"
    staging_release_bucket     = "aidash-nonproduction-releases"
    production_backup_enabled  = true
    production_backup_location = "us-east1"
  }
  assert {
    condition = (
      google_gke_backup_backup_plan.pre_update[0].location == "us-east1" &&
      google_gke_backup_restore_plan.pre_update_in_place[0].location == "us-central1" &&
      google_gke_backup_restore_plan.pre_update_in_place[0].cluster == "projects/aidash-fixture/locations/us-central1/clusters/aidash"
    )
    error_message = "The restore plan must live in the target cluster's region while the backup plan stays in the cross-region backup location."
  }
}
run "backup_requires_existing_enabled_agent" {
  command = plan
  variables {
    deployment_tier            = "production"
    nonproduction_project_id   = "aidash-nonproduction-fixture"
    nonproduction_state_bucket = "aidash-nonproduction-state"
    staging_release_bucket     = "aidash-nonproduction-releases"
    production_backup_enabled  = true
  }
  override_data {
    target = data.google_container_cluster.production_operations[0]
    values = { addons_config = [{ gke_backup_agent_config = [{ enabled = false }] }] }
  }
  expect_failures = [google_gke_backup_backup_plan.production[0], google_gke_backup_backup_plan.pre_update[0]]
}
run "nonproduction_cannot_enable_production_operations" {
  command = plan
  variables {
    production_backup_enabled     = true
    production_monitoring_enabled = true
  }
  expect_failures = [var.production_backup_enabled, var.production_monitoring_enabled]
}
run "delete_lock_cannot_outlive_backup_retention" {
  command = plan
  variables {
    production_backup_retain_days      = 7
    production_backup_delete_lock_days = 8
  }
  expect_failures = [var.production_backup_delete_lock_days]
}
run "pre_update_delete_lock_cannot_outlive_retention" {
  command = plan
  variables {
    production_pre_update_backup_retain_days      = 2
    production_pre_update_backup_delete_lock_days = 3
  }
  expect_failures = [var.production_pre_update_backup_delete_lock_days]
}
run "delivery_cannot_publish_missing_preview_tls_secret" {
  command = plan
  variables {
    delivery_configuration = {
      cloudflare_zone_id    = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
      release_app_bot_login = "aidash-release[bot]"
    }
  }
  expect_failures = [var.delivery_configuration]
}

run "production_agent_preparation_precedes_cluster_and_backup_plan" {
  command = plan
  variables {
    deployment_tier                 = "production"
    nonproduction_project_id        = "aidash-nonproduction-fixture"
    nonproduction_state_bucket      = "aidash-nonproduction-state"
    staging_release_bucket          = "aidash-nonproduction-releases"
    production_backup_agent_enabled = true
  }
  assert {
    condition     = contains(keys(google_project_service.production_operations), "gkebackup.googleapis.com") && output.operations_configuration.gke_backup_enabled && length(data.google_container_cluster.production_operations) == 0 && length(google_gke_backup_backup_plan.production) == 0
    error_message = "Agent preparation must enable the API and transport flag without querying a missing cluster or creating a backup plan."
  }
}
run "nonproduction_cannot_prepare_production_backup_agent" {
  command = plan
  variables { production_backup_agent_enabled = true }
  expect_failures = [var.production_backup_agent_enabled]
}
