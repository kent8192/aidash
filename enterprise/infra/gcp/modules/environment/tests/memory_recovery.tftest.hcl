mock_provider "google" {
  mock_resource "google_service_account" {
    defaults = { name = "projects/aidash-fixture/serviceAccounts/fixture-server@aidash-fixture.iam.gserviceaccount.com", email = "fixture-server@aidash-fixture.iam.gserviceaccount.com" }
  }
}
variables {
  project_id           = "aidash-fixture"
  deployment_tier      = "production"
  environment_id       = "production"
  hostname             = "aidash.run"
  node_service_account = "nodes@aidash-fixture.iam.gserviceaccount.com"
  cluster              = { name = "aidash", location = "us-central1", workload_pool = "aidash-fixture.svc.id.goog", node_locations = ["us-central1-a"] }
  environment          = { kind = "production", incarnation = "aaaaaaaaaaaa", running = true, spot = false, nodes = 1, machine_type = "n2-standard-4" }
}
run "nonproduction_is_explicit_opt_in" {
  command = plan
  variables {
    deployment_tier = "nonproduction"
    environment_id  = "test"
    environment     = { kind = "test", incarnation = "aaaaaaaaaaaa", running = true, spot = true, nodes = 1, machine_type = "n2-standard-4" }
  }
  assert {
    condition     = !output.memory_recovery.required && length(google_storage_bucket.memory_history) == 0 && length(google_service_account.memory_jobs) == 0
    error_message = "Ordinary local/nonproduction profiles remain opt-in."
  }
}
run "production_keeps_history_without_body_retention_leftovers" {
  command = apply
  assert {
    condition = (
      output.memory_recovery.required &&
      google_storage_bucket.memory_history[0].location == "US-CENTRAL1" &&
      google_storage_bucket.memory_history[0].uniform_bucket_level_access &&
      google_storage_bucket.memory_history[0].public_access_prevention == "enforced" &&
      !google_storage_bucket.memory_history[0].force_destroy &&
      length(google_storage_bucket.memory_history[0].lifecycle_rule) == 0 &&
      google_storage_bucket.memory_archives[0].soft_delete_policy[0].retention_duration_seconds == 0 &&
      !google_storage_bucket.memory_archives[0].versioning[0].enabled &&
      !google_storage_bucket.memory_archives[0].default_event_based_hold &&
      length(google_storage_bucket.memory_archives[0].retention_policy) == 0 &&
      !google_storage_bucket.memory_archives[0].force_destroy
    )
    error_message = "Production must keep its non-body history independently and allow actual expiry of body copies."
  }
  assert {
    condition = (
      toset(keys(google_storage_bucket_iam_member.memory_history_writer)) == toset(["server", "worker"]) &&
      google_storage_bucket_iam_member.memory_backup_history[0].role == "roles/storage.objectViewer" &&
      google_storage_bucket_iam_member.memory_backup_bodies[0].role == "projects/aidash-fixture/roles/aidashMemoryHistory" &&
      google_storage_bucket_iam_member.memory_prune_bodies[0].role == "projects/aidash-fixture/roles/aidashMemoryPrune" &&
      length(google_service_account.memory_jobs) == 2 &&
      google_service_account_iam_member.memory_jobs["backup"].member == "serviceAccount:aidash-fixture.svc.id.goog[aidash-production/app-aidash-memory-backup]" &&
      google_service_account_iam_member.memory_jobs["prune"].member == "serviceAccount:aidash-fixture.svc.id.goog[aidash-production/app-aidash-memory-prune]"
    )
    error_message = "Safety writers cannot delete or overwrite; backup and prune have distinct bucket-scoped identities."
  }
}
run "maximum_project_ids_keep_bucket_names_valid" {
  command   = plan
  state_key = "maximum-project-id"
  variables { project_id = "paaaaaaaaaaaaaaaaaaaaaaaaaaaaa" }
  assert {
    condition = (
      length(google_storage_bucket.memory_history[0].name) <= 63 &&
      length(google_storage_bucket.memory_archives[0].name) <= 63 &&
      google_storage_bucket.memory_history[0].name != google_storage_bucket.memory_archives[0].name
    )
    error_message = "Valid 30-character project IDs must produce valid distinct GCS bucket names."
  }
}
