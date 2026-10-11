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
  assert {
    condition     = google_identity_platform_tenant_inbound_saml_config.saml["sso/saml.company"].sp_config[0].callback_uri == "https://aidash-fixture.firebaseapp.com/__/auth/handler" && output.gcip.auth_helper == "firebase"
    error_message = "Until a release deployment selects the edge's helper proxy, SAML must post to Firebase's helper."
  }
  assert {
    condition     = var.gcip_tenants["password"].mfa.state == "disabled" && var.gcip_tenants["sso"].mfa.state == "disabled"
    error_message = "An omitted MFA Requirement must be declared disabled."
  }
}
run "saml_posts_to_the_proxied_helper_with_its_release" {
  command = plan
  variables {
    environment = {
      kind = "test", incarnation = "aaaaaaaaaaaa", running = false, spot = false, nodes = 1, machine_type = "n2-standard-4", auth_helper = "public_origin"
    }
    gcip_tenants     = { sso = { tenant = "company", password_sign_up = false, saml = { "saml.company" = { idp_entity_id = "company", sso_url = "https://sso.example.test", sp_entity_id = "aidash", certificates = ["fixture-certificate"] } } } }
    gcip_idp_secrets = {}
  }
  assert {
    condition     = google_identity_platform_tenant_inbound_saml_config.saml["sso/saml.company"].sp_config[0].callback_uri == "https://test.aidash.run/__/auth/handler" && output.gcip.auth_helper == "public_origin"
    error_message = "SAML must post to this environment's proxied sign-in helper, which the runtime is told to use."
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
run "sign_in_domain_cannot_be_an_ipv4_literal" {
  command = plan
  variables { gcip_tenants = { a = { tenant = "a", sign_in_domains = ["192.0.2.1"] } } }
  expect_failures = [var.gcip_tenants]
}
run "sign_in_domain_cannot_end_in_a_number" {
  command = plan
  variables { gcip_tenants = { a = { tenant = "a", sign_in_domains = ["acme.0x1f"] } } }
  expect_failures = [var.gcip_tenants]
}
run "sign_in_domain_may_have_numeric_inner_labels" {
  command = plan
  variables { gcip_tenants = { a = { tenant = "a", sign_in_domains = ["192.0.2.example"] } } }
  assert {
    condition     = keys(output.gcip.sign_in_domains) == ["192.0.2.example"]
    error_message = "Only the final label decides whether a host is an IP literal."
  }
}
run "password_pool_cannot_enable_mfa_before_clients_support_it" {
  command = plan
  variables { gcip_tenants = { acme = { tenant = "acme", mfa = { state = "enabled" } } } }
  expect_failures = [var.gcip_tenants]
}
