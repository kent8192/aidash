mock_provider "google" {}
variables {
  project_id          = "aidash-fixture"
  byok_project_id     = "aidash-byok-fixture"
  state_bucket_name   = "aidash-fixture-state"
  release_bucket_name = "aidash-fixture-releases"
}
run "trusted_workflow_only" {
  command = plan
  assert {
    condition     = strcontains(google_iam_workload_identity_pool_provider.github.attribute_condition, "assertion.repository_id == '1378229915'") && strcontains(google_iam_workload_identity_pool_provider.github.attribute_condition, "gcp-environments.yml@refs/heads/main") && strcontains(google_iam_workload_identity_pool_provider.github.attribute_condition, "assertion.ref == 'refs/heads/main'")
    error_message = "Name-only or PR-branch OIDC trust is not acceptable."
  }
  assert {
    condition     = google_storage_bucket.state.public_access_prevention == "enforced" && google_storage_bucket.state.force_destroy == false
    error_message = "State must remain private and protected from accidental destruction."
  }
}
