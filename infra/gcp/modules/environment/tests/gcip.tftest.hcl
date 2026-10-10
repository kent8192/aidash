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
run "sign_in_domains_route_to_pool" {
  command = plan
  variables { gcip_tenants = { acme = { tenant = "acme", sign_in_domains = ["acme.example", "xn--bcher-kva.example"] } } }
  assert {
    condition     = keys(output.gcip.sign_in_domains) == ["acme.example", "xn--bcher-kva.example"]
    error_message = "Each Sign-in Domain must map to its pool."
  }
}
run "sign_in_domain_cannot_route_to_two_pools" {
  command = plan
  variables { gcip_tenants = { a = { tenant = "a", sign_in_domains = ["acme.example"] }, b = { tenant = "b", sign_in_domains = ["acme.example"] } } }
  expect_failures = [var.gcip_tenants]
}
run "sign_in_domain_must_be_lowercase" {
  command = plan
  variables { gcip_tenants = { a = { tenant = "a", sign_in_domains = ["Acme.example"] } } }
  expect_failures = [var.gcip_tenants]
}
run "sign_in_domain_needs_two_labels" {
  command = plan
  variables { gcip_tenants = { a = { tenant = "a", sign_in_domains = ["localhost"] } } }
  expect_failures = [var.gcip_tenants]
}
