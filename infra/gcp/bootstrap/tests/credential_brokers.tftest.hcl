mock_provider "google" {
  mock_resource "google_service_account" {
    defaults = {
      name  = "projects/aidash-fixture/serviceAccounts/automation@aidash-fixture.iam.gserviceaccount.com"
      email = "automation@aidash-fixture.iam.gserviceaccount.com"
    }
  }
  mock_data "google_project" { defaults = { number = "123456789012" } }
}
variables {
  project_id          = "aidash-fixture"
  byok_project_id     = "aidash-byok-fixture"
  state_bucket_name   = "aidash-fixture-state"
  release_bucket_name = "aidash-fixture-releases"
}
run "broker_environments" {
  command = apply
  variables { byok_broker_environments = ["production", "test"] }
  override_resource {
    target = google_service_account.broker["production"]
    values = { email = "aidash-production-broker@aidash-fixture.iam.gserviceaccount.com" }
  }
  override_resource {
    target = google_service_account.broker["test"]
    values = { email = "aidash-test-broker@aidash-fixture.iam.gserviceaccount.com" }
  }
  assert {
    condition     = alltrue([for id, account in google_service_account.broker : account.project == var.project_id && account.account_id == "aidash-${id}-broker" && google_project_iam_member.byok_broker_read[id].project == var.byok_project_id && google_project_iam_member.byok_broker_read[id].role == google_project_iam_custom_role.byok_broker_read[0].name && google_project_iam_member.byok_broker_read[id].member == "serviceAccount:${account.email}" && google_project_iam_member.byok_broker_read[id].condition[0].expression == "resource.name.startsWith(\"projects/123456789012/secrets/aidash-${id}-cred-\")"])
    error_message = "Only dedicated broker SAs may receive prefix-conditioned BYOK read access."
  }
  assert {
    condition     = output.broker_service_accounts == { production = "aidash-production-broker@aidash-fixture.iam.gserviceaccount.com", test = "aidash-test-broker@aidash-fixture.iam.gserviceaccount.com" }
    error_message = "Export the per-environment broker identities for deployment."
  }
}
run "brokers_opt_in_only" {
  command = apply
  assert {
    condition     = length(google_service_account.broker) == 0 && length(google_project_iam_member.byok_broker_read) == 0
    error_message = "No broker identity or read grant without explicit opt-in."
  }
}
run "without_provider_store" {
  command = plan
  variables {
    byok_project_id          = ""
    byok_broker_environments = ["test"]
  }
  assert {
    condition     = length(google_service_account.broker) == 0 && length(google_project_iam_member.byok_broker_read) == 0 && output.broker_service_accounts == {}
    error_message = "Disabled BYOK must create no broker identities or payload-access grants even with an environment opt-in."
  }
}
run "bootstrap_preview_broker_forbidden" {
  command = plan
  variables { byok_broker_environments = ["pr-137"] }
  expect_failures = [var.byok_broker_environments]
}
run "longest_broker_identity_fits_gcp_account_id" {
  command = plan
  variables { byok_broker_environments = ["abcdefghijklmnop"] }
  assert {
    condition     = length(google_service_account.broker["abcdefghijklmnop"].account_id) == 30
    error_message = "The 16-character environment boundary must fit GCP's 30-character account ID."
  }
}
run "bootstrap_17_character_broker_forbidden" {
  command = plan
  variables { byok_broker_environments = ["abcdefghijklmnopq"] }
  expect_failures = [var.byok_broker_environments]
}
