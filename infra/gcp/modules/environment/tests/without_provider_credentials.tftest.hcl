mock_provider "google" {}

variables {
  project_id           = "aidash-fixture"
  node_service_account = "aidash-gke-nodes@aidash-fixture.iam.gserviceaccount.com"
  cluster = {
    name          = "aidash"
    location      = "us-central1-a"
    workload_pool = "aidash-fixture.svc.id.goog"
  }
  environment_id = "test"
  hostname       = "test.aidash.run"
  environment = {
    kind         = "test"
    incarnation  = "aaaaaaaaaaaa"
    running      = false
    spot         = true
    nodes        = 1
    machine_type = "n2-standard-4"
  }
}

run "legacy_environment_has_no_byok_grants" {
  command = plan
  assert {
    condition = (
      length(data.google_project.byok) == 0 &&
      length(google_project_iam_member.provider_credential_create) == 0 &&
      length(google_project_iam_member.provider_credential_manage) == 0 &&
      output.byok_project_id == "" && output.secret_prefix == "" &&
      output.provider_credentials == null &&
      google_container_node_pool.environment.node_count == 0
    )
    error_message = "Omitting BYOK must keep the environment without project lookups, IAM grants or a Store descriptor."
  }
}
