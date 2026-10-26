mock_provider "google" {}
variables {
  project_id               = "aidash-fixture"
  state_bucket_name        = "aidash-fixture-state"
  release_bucket_name      = "aidash-fixture-releases"
  notification_bucket_name = "aidash-fixture-notifications"
}
run "gcip_domains_and_multitenancy" {
  command = plan
  variables {
    gcip_enabled        = true
    environment_domains = ["aidash.run", "example.test"]
  }
  assert {
    condition     = google_identity_platform_config.aidash[0].multi_tenant[0].allow_tenants && toset(google_identity_platform_config.aidash[0].authorized_domains) == toset(["aidash-fixture.firebaseapp.com", "test.aidash.run", "test.example.test"])
    error_message = "Bootstrap initializes only Firebase and fixed test hosts; the controller registers exact random preview hosts."
  }
}

run "production_gcip_does_not_authorize_preview_hosts" {
  command = plan
  variables {
    deployment_tier            = "production"
    nonproduction_project_id   = "aidash-nonproduction-fixture"
    nonproduction_state_bucket = "aidash-nonproduction-state"
    staging_release_bucket     = "aidash-nonproduction-releases"
    gcip_enabled               = true
  }
  assert {
    condition     = toset(google_identity_platform_config.aidash[0].authorized_domains) == toset(["aidash-fixture.firebaseapp.com", "aidash.run"])
    error_message = "Production GCIP accepts its fixed public host and isolated Firebase helper only."
  }
}
