mock_provider "google" {}
variables {
  project_id             = "aidash-fixture"
  release_bucket         = "aidash-fixture-releases"
  deploy_service_account = "deploy@aidash-fixture.iam.gserviceaccount.com"
  environment_id         = "test"
  hostname               = "test.aidash.run"
  environment = {
    kind          = "test", incarnation = "aaaaaaaaaaaa", generation = 1, running = false, published = false, spot = false, vm_present = true,
    machine_type  = "e2-standard-4", boot_disk_gib = 10, data_disk_gib = 20,
    bundle_object = "bundles/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.tar.gz", bundle_sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", release_sha = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
  }
}
run "password_and_sso_tenants_are_separate" {
  command = plan
  variables {
    gcip_tenants = {
      password = { tenant = "acme", password_sign_up = true, google_client_id = "google-client" }
      sso      = { tenant = "company", password_sign_up = false, oidc = { "oidc.company" = { issuer = "https://issuer.example.test", client_id = "company" } }, saml = { "saml.company" = { idp_entity_id = "company", sso_url = "https://sso.example.test", sp_entity_id = "aidash", certificates = ["fixture-certificate"] } } }
    }
    gcip_idp_secrets = { "password/google.com" = "fixture-google", "sso/oidc.company" = "fixture-oidc" }
  }
  assert {
    condition     = google_identity_platform_tenant.aidash["password"].allow_password_signup && !google_identity_platform_tenant.aidash["sso"].allow_password_signup && google_identity_platform_tenant.aidash["sso"].client[0].permissions[0].disabled_user_signup
    error_message = "Password signup is per pool and must be disabled for SSO pools."
  }
  assert {
    condition     = length(google_identity_platform_tenant_default_supported_idp_config.google) == 1 && length(google_identity_platform_tenant_oauth_idp_config.oidc) == 1 && length(google_identity_platform_tenant_inbound_saml_config.saml) == 1
    error_message = "Each selected IdP must be configured inside its environment's tenant."
  }
}
run "sso_cannot_enable_password_signup" {
  command = plan
  variables { gcip_tenants = { company = { tenant = "company", password_sign_up = true, oidc = { "oidc.company" = { issuer = "https://issuer.example.test", client_id = "company" } } } } }
  expect_failures = [var.gcip_tenants]
}
