mock_provider "google" {}

variables {
  project_id             = "aidash-fixture"
  release_bucket         = "aidash-fixture-releases"
  deploy_service_account = "deploy@aidash-fixture.iam.gserviceaccount.com"
  environment_id         = "test"
  hostname               = "test.aidash.run"
  environment = {
    kind          = "test"
    incarnation   = "aaaaaaaaaaaa"
    generation    = 1
    running       = false
    published     = false
    spot          = true
    vm_present    = true
    machine_type  = "e2-standard-4"
    boot_disk_gib = 10
    data_disk_gib = 20
    bundle_object = "bundles/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.tar.gz"
    bundle_sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    release_sha   = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
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
      google_compute_disk.data.size == 20
    )
    error_message = "Omitting BYOK must preserve retained infrastructure without project lookups or IAM grants."
  }
}
