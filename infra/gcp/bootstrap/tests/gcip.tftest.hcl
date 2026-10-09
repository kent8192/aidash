mock_provider "google" {}
variables {
  project_id          = "aidash-fixture"
  state_bucket_name   = "aidash-fixture-state"
  release_bucket_name = "aidash-fixture-releases"
}
run "gcip_domains_and_multitenancy" {
  command = plan
  variables {
    gcip_enabled        = true
    environment_domains = ["aidash.run", "example.test"]
  }
  assert {
    condition     = google_identity_platform_config.aidash[0].multi_tenant[0].allow_tenants && contains(google_identity_platform_config.aidash[0].authorized_domains, "preview.aidash.run") && contains(google_identity_platform_config.aidash[0].authorized_domains, "test.example.test")
    error_message = "GCIP must enable multi-tenancy and derive callback hosts from environment domains."
  }
}
