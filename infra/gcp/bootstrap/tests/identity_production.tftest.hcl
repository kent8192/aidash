mock_provider "google" {
  mock_resource "google_service_account" {
    defaults = {
      name  = "projects/aidash-fixture/serviceAccounts/fixture@aidash-fixture.iam.gserviceaccount.com"
      email = "fixture@aidash-fixture.iam.gserviceaccount.com"
    }
  }
}
variables {
  project_id               = "aidash-fixture"
  byok_project_id          = "aidash-byok-fixture"
  state_bucket_name        = "aidash-fixture-state"
  release_bucket_name      = "aidash-fixture-releases"
  notification_bucket_name = "aidash-fixture-notifications"
  security_alert_emails    = ["security@example.test"]
}
// Production uses a separate project/state; never destroy protected nonproduction secrets.
run "production_workflow_is_main_only" {
  command = plan
  variables {
    deployment_tier            = "production"
    staging_release_bucket     = "aidash-nonproduction-releases"
    nonproduction_project_id   = "aidash-nonproduction-fixture"
    nonproduction_state_bucket = "aidash-nonproduction-state"
    byok_broker_environments   = ["production"]
  }
  assert {
    condition = (
      strcontains(google_iam_workload_identity_pool_provider.github.attribute_condition, "gcp-production.yml@refs/heads/main") &&
      strcontains(google_iam_workload_identity_pool_provider.github.attribute_condition, "assertion.ref == 'refs/heads/main'") &&
      strcontains(google_iam_workload_identity_pool_provider.github.attribute_condition, "assertion.environment == 'production'") &&
      !strcontains(google_iam_workload_identity_pool_provider.github.attribute_condition, "gcp-environments.yml") &&
      keys(google_service_account.broker) == ["production"] &&
      google_service_account.production_nodes[0].account_id == "aidash-gke-nodes" &&
      google_artifact_registry_repository_iam_member.production_nodes_staging[0].project == "aidash-nonproduction-fixture" &&
      google_artifact_registry_repository_iam_member.production_nodes_staging[0].role == "roles/artifactregistry.reader" &&
      google_storage_bucket_iam_member.staging_promotion_reader[0].role == "roles/storage.objectViewer" &&
      google_storage_bucket_iam_member.staging_promotion_reader[0].condition[0].expression == "resource.name.startsWith('projects/_/buckets/aidash-nonproduction-releases/objects/deployments/develop/')"
    )
    error_message = "Only the approved main production workflow may deploy the isolated production project."
  }
}
run "production_broker_cannot_live_in_nonproduction" {
  command = plan
  variables { byok_broker_environments = ["production"] }
  expect_failures = [var.byok_broker_environments]
}
run "production_project_cannot_equal_nonproduction" {
  command = plan
  variables {
    deployment_tier            = "production"
    staging_release_bucket     = "aidash-nonproduction-releases"
    nonproduction_project_id   = "aidash-fixture"
    nonproduction_state_bucket = "aidash-nonproduction-state"
  }
  expect_failures = [var.nonproduction_project_id]
}
run "production_state_cannot_equal_nonproduction" {
  command = plan
  variables {
    deployment_tier            = "production"
    staging_release_bucket     = "aidash-nonproduction-releases"
    nonproduction_project_id   = "aidash-nonproduction-fixture"
    nonproduction_state_bucket = "aidash-fixture-state"
  }
  expect_failures = [var.nonproduction_state_bucket]
}
