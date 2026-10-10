mock_provider "google" {}
variables {
  project_id           = "aidash-fixture"
  node_service_account = "aidash-gke-nodes@aidash-fixture.iam.gserviceaccount.com"
  cluster              = { name = "aidash", location = "us-central1-a", workload_pool = "aidash-fixture.svc.id.goog" }
  environment_id       = "test"
  hostname             = "test.aidash.run"
  environment = {
    kind = "test", incarnation = "aaaaaaaaaaaa", running = false, spot = false, nodes = 1, machine_type = "n2-standard-4"
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
    condition     = google_identity_platform_tenant.aidash["password"].allow_password_signup && !google_identity_platform_tenant.aidash["sso"].allow_password_signup && !google_identity_platform_tenant.aidash["sso"].client[0].permissions[0].disabled_user_signup && !google_identity_platform_tenant.aidash["password"].client[0].permissions[0].disabled_user_signup
    error_message = "SSO pools must disable passwords while allowing federated first sign-in to create an account."
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
