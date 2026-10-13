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
      google_gke_backup_backup_plan.production[0].backup_config[0].selected_namespaces[0].namespaces == tolist(["aidash-production"]) &&
      google_gke_backup_backup_plan.production[0].backup_config[0].include_volume_data &&
      google_gke_backup_backup_plan.production[0].backup_config[0].include_secrets &&
      !google_gke_backup_backup_plan.production[0].backup_config[0].permissive_mode &&
      google_gke_backup_backup_plan.production[0].retention_policy[0].backup_retain_days == 30 &&
      google_gke_backup_backup_plan.production[0].retention_policy[0].backup_delete_lock_days == 7 &&
      !google_gke_backup_backup_plan.production[0].retention_policy[0].locked &&
      google_gke_backup_backup_plan.production[0].deletion_policy == "PREVENT"
    )
    error_message = "Only production namespace/config/PVCs are backed up with explicit retention and deletion protection."
  }
  assert {
    condition = (
      google_monitoring_uptime_check_config.production[0].http_check[0].use_ssl &&
      google_monitoring_uptime_check_config.production[0].http_check[0].validate_ssl &&
      google_monitoring_uptime_check_config.production[0].http_check[0].path == "/health" &&
      google_monitoring_uptime_check_config.production[0].content_matchers[0].content == "aidash://aidash-production-" &&
      google_monitoring_notification_channel.production[0].labels.email_address == "prod.deployment@aidash.run" &&
      google_monitoring_alert_policy.production_uptime[0].conditions[0].condition_threshold[0].trigger[0].count == 2 &&
      length(output.operations_configuration.alert_policies) == 4 &&
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
  expect_failures = [google_gke_backup_backup_plan.production[0]]
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
