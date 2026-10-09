mock_provider "google" {}

variables {
  project_id          = "aidash-fixture"
  state_bucket_name   = "aidash-fixture-state"
  release_bucket_name = "aidash-fixture-releases"
}

run "legacy_bootstrap_has_no_byok_resources" {
  command = plan
  assert {
    condition = (
      length(data.google_project.byok) == 0 &&
      length(google_project_service.byok_secret_manager) == 0 &&
      length(google_project_iam_custom_role.byok_create) == 0 &&
      length(google_project_iam_custom_role.byok_manage) == 0 &&
      length(google_project_iam_custom_role.byok_broker_read) == 0 &&
      length(google_project_iam_custom_role.byok_deploy) == 0 &&
      length(google_project_iam_member.byok_deploy) == 0 &&
      length(google_project_iam_audit_config.byok_secret_manager) == 0
    )
    error_message = "An omitted BYOK project must leave existing bootstrap usable without BYOK resources."
  }
}

run "shared_project_cannot_be_used_for_byok" {
  command = plan
  variables { byok_project_id = "aidash-fixture" }
  expect_failures = [var.byok_project_id]
}
