mock_provider "google" {
  mock_resource "google_service_account" {
    defaults = {
      name  = "projects/aidash-fixture/serviceAccounts/automation@aidash-fixture.iam.gserviceaccount.com"
      email = "automation@aidash-fixture.iam.gserviceaccount.com"
    }
  }
  mock_resource "google_tags_tag_key" {
    defaults = { id = "tagKeys/1001", namespaced_name = "aidash-fixture/aidash-credential-broker" }
  }
  mock_resource "google_tags_tag_value" {
    defaults = { id = "tagValues/2001" }
  }
  mock_data "google_project" { defaults = { number = "123456789012" } }
}
variables {
  project_id               = "aidash-fixture"
  byok_project_id          = "aidash-byok-fixture"
  state_bucket_name        = "aidash-fixture-state"
  release_bucket_name      = "aidash-fixture-releases"
  notification_bucket_name = "aidash-fixture-notifications"
  security_alert_emails    = ["security@example.test"]
}
run "broker_environments" {
  command   = apply
  state_key = "broker-environments"
  variables { byok_broker_environments = ["develop", "test"] }
  override_resource {
    target = google_service_account.automation["deploy"]
    values = {
      email = "aidash-deploy@aidash-fixture.iam.gserviceaccount.com"
      name  = "projects/aidash-fixture/serviceAccounts/aidash-deploy@aidash-fixture.iam.gserviceaccount.com"
    }
  }
  override_resource {
    target = google_service_account.broker["develop"]
    values = {
      email     = "aidash-develop-broker@aidash-fixture.iam.gserviceaccount.com"
      name      = "projects/aidash-fixture/serviceAccounts/aidash-develop-broker@aidash-fixture.iam.gserviceaccount.com"
      unique_id = "10000000000000000001"
    }
  }
  override_resource {
    target = google_service_account.broker["test"]
    values = {
      email     = "aidash-test-broker@aidash-fixture.iam.gserviceaccount.com"
      name      = "projects/aidash-fixture/serviceAccounts/aidash-test-broker@aidash-fixture.iam.gserviceaccount.com"
      unique_id = "10000000000000000002"
    }
  }
  override_resource {
    target = google_service_account.tenant_secret_broker["develop"]
    values = {
      email     = "aidash-develop-tsec@aidash-fixture.iam.gserviceaccount.com"
      name      = "projects/aidash-fixture/serviceAccounts/aidash-develop-tsec@aidash-fixture.iam.gserviceaccount.com"
      unique_id = "10000000000000000003"
    }
  }
  override_resource {
    target = google_service_account.tenant_secret_broker["test"]
    values = {
      email     = "aidash-test-tsec@aidash-fixture.iam.gserviceaccount.com"
      name      = "projects/aidash-fixture/serviceAccounts/aidash-test-tsec@aidash-fixture.iam.gserviceaccount.com"
      unique_id = "10000000000000000004"
    }
  }
  override_resource {
    target = google_kms_key_ring.capability["develop"]
    values = { id = "projects/aidash-fixture/locations/us-central1/keyRings/aidash-develop-capability" }
  }
  override_resource {
    target = google_kms_crypto_key.capability["develop"]
    values = { id = "projects/aidash-fixture/locations/us-central1/keyRings/aidash-develop-capability/cryptoKeys/capability" }
  }
  override_resource {
    target = google_kms_crypto_key.tenant_secret["develop"]
    values = { id = "projects/aidash-fixture/locations/us-central1/keyRings/aidash-develop-capability/cryptoKeys/tenant-secret" }
  }
  override_resource {
    target = google_kms_key_ring.capability["test"]
    values = { id = "projects/aidash-fixture/locations/us-central1/keyRings/aidash-test-capability" }
  }
  override_resource {
    target = google_kms_crypto_key.capability["test"]
    values = { id = "projects/aidash-fixture/locations/us-central1/keyRings/aidash-test-capability/cryptoKeys/capability" }
  }
  override_resource {
    target = google_kms_crypto_key.tenant_secret["test"]
    values = { id = "projects/aidash-fixture/locations/us-central1/keyRings/aidash-test-capability/cryptoKeys/tenant-secret" }
  }
  assert {
    condition     = alltrue([for id, account in google_service_account.tenant_secret_broker : google_project_iam_member.byok_oauth_broker[id].member == "serviceAccount:${account.email}" && google_project_iam_member.byok_oauth_broker[id].condition[0].expression == "resource.name.startsWith(\"projects/123456789012/secrets/aidash-${id}-oauth-\")"]) && google_project_iam_custom_role.byok_oauth_broker[0].permissions == toset(["secretmanager.versions.access", "secretmanager.versions.add"])
    error_message = "Only the Tenant Secret broker can read/write OAuth versions, scoped to its own -oauth- prefix."
  }

  assert {
    condition     = alltrue([for id, account in google_service_account.broker : account.project == var.project_id && account.account_id == "aidash-${id}-broker" && google_project_iam_member.byok_broker_read[id].project == var.byok_project_id && google_project_iam_member.byok_broker_read[id].role == google_project_iam_custom_role.byok_broker_read[0].name && google_project_iam_member.byok_broker_read[id].member == "serviceAccount:${account.email}" && google_project_iam_member.byok_broker_read[id].condition[0].expression == "resource.name.startsWith(\"projects/123456789012/secrets/aidash-${id}-cred-\")"])
    error_message = "Only dedicated broker SAs may receive prefix-conditioned BYOK read access."
  }
  assert {
    condition     = alltrue([for id, account in google_service_account.tenant_secret_broker : account.project == var.project_id && account.account_id == "aidash-${id}-tsec" && google_project_iam_member.byok_tenant_secret_broker_read[id].project == var.byok_project_id && google_project_iam_member.byok_tenant_secret_broker_read[id].role == google_project_iam_custom_role.byok_tenant_secret_broker_read[0].name && google_project_iam_member.byok_tenant_secret_broker_read[id].member == "serviceAccount:${account.email}" && google_project_iam_member.byok_tenant_secret_broker_read[id].condition[0].expression == "resource.name.startsWith(\"projects/123456789012/secrets/aidash-${id}-tsec-\")"])
    error_message = "The Tenant Secret broker reads only its environment's -tsec- prefix and never Key Material."
  }
  assert {
    condition     = keys(google_service_account_iam_member.deploy_act_as_broker) == ["develop", "test"] && alltrue([for id, grant in google_service_account_iam_member.deploy_act_as_broker : grant.role == "roles/iam.serviceAccountUser" && grant.member == "serviceAccount:aidash-deploy@aidash-fixture.iam.gserviceaccount.com" && grant.service_account_id == google_service_account.broker[id].name])
    error_message = "Bootstrap, not deploy automation, grants deploy actAs on each broker SA."
  }
  assert {
    condition     = keys(google_service_account_iam_member.deploy_act_as_tenant_secret_broker) == ["develop", "test"] && alltrue([for id, grant in google_service_account_iam_member.deploy_act_as_tenant_secret_broker : grant.role == "roles/iam.serviceAccountUser" && grant.member == "serviceAccount:aidash-deploy@aidash-fixture.iam.gserviceaccount.com" && grant.service_account_id == google_service_account.tenant_secret_broker[id].name])
    error_message = "Bootstrap, not deploy automation, grants deploy actAs on each Tenant Secret broker SA."
  }
  assert {
    condition     = output.broker_service_accounts == { develop = "aidash-develop-broker@aidash-fixture.iam.gserviceaccount.com", test = "aidash-test-broker@aidash-fixture.iam.gserviceaccount.com" } && output.tenant_secret_broker_service_accounts == { develop = "aidash-develop-tsec@aidash-fixture.iam.gserviceaccount.com", test = "aidash-test-tsec@aidash-fixture.iam.gserviceaccount.com" }
    error_message = "Export the per-environment broker and Tenant Secret broker identities for deployment."
  }
  assert {
    condition     = alltrue([for id, key in google_kms_crypto_key.capability : key.purpose == "ASYMMETRIC_SIGN" && key.name == "capability" && key.key_ring == google_kms_key_ring.capability[id].id && key.version_template[0].algorithm == "EC_SIGN_ED25519" && google_kms_key_ring.capability[id].name == "aidash-${id}-capability" && google_kms_key_ring.capability[id].project == var.project_id]) && keys(output.broker_signing_keys) == ["develop", "test"]
    error_message = "Bootstrap alone owns and exports the permanent per-environment Ed25519 signing keys."
  }
  assert {
    condition     = alltrue([for id, key in google_kms_crypto_key.tenant_secret : key.purpose == "ASYMMETRIC_SIGN" && key.name == "tenant-secret" && key.key_ring == google_kms_key_ring.capability[id].id && key.version_template[0].algorithm == "EC_SIGN_ED25519"]) && keys(output.tenant_secret_signing_keys) == ["develop", "test"]
    error_message = "Bootstrap alone owns the server's separate Ed25519 Tenant Secret signing key."
  }
  assert {
    condition     = keys(google_kms_crypto_key_iam_member.deploy_capability_public_key) == ["develop", "test"] && alltrue([for id, grant in google_kms_crypto_key_iam_member.deploy_capability_public_key : grant.crypto_key_id == google_kms_crypto_key.capability[id].id && grant.role == "roles/cloudkms.publicKeyViewer" && grant.member == "serviceAccount:aidash-deploy@aidash-fixture.iam.gserviceaccount.com"])
    error_message = "Bootstrap grants deploy only key-scoped public-key reads on each permanent signing key."
  }
  assert {
    condition     = keys(google_kms_crypto_key_iam_member.deploy_tenant_secret_public_key) == ["develop", "test"] && alltrue([for id, grant in google_kms_crypto_key_iam_member.deploy_tenant_secret_public_key : grant.crypto_key_id == google_kms_crypto_key.tenant_secret[id].id && grant.role == "roles/cloudkms.publicKeyViewer" && grant.member == "serviceAccount:aidash-deploy@aidash-fixture.iam.gserviceaccount.com"])
    error_message = "Bootstrap grants deploy only key-scoped public-key reads on the server signing key."
  }
}
run "brokers_opt_in_only" {
  command = apply
  assert {
    condition     = length(google_service_account.broker) == 0 && length(google_service_account.tenant_secret_broker) == 0 && length(google_service_account_iam_member.deploy_act_as_broker) == 0 && length(google_service_account_iam_member.deploy_act_as_tenant_secret_broker) == 0 && length(google_project_iam_member.byok_broker_read) == 0 && length(google_project_iam_member.byok_tenant_secret_broker_read) == 0 && length(google_project_iam_member.byok_oauth_broker) == 0 && length(google_kms_crypto_key.capability) == 0 && length(google_kms_crypto_key.tenant_secret) == 0 && length(google_kms_crypto_key_iam_member.deploy_capability_public_key) == 0 && length(google_kms_crypto_key_iam_member.deploy_tenant_secret_public_key) == 0
    error_message = "No broker identity, Tenant Secret identity or read grant without explicit opt-in."
  }
}
run "without_provider_store" {
  command = plan
  variables {
    byok_project_id          = ""
    byok_broker_environments = ["test"]
  }
  assert {
    condition     = length(google_service_account.broker) == 0 && length(google_service_account.tenant_secret_broker) == 0 && length(google_project_iam_member.byok_broker_read) == 0 && length(google_project_iam_member.byok_tenant_secret_broker_read) == 0 && length(google_project_iam_member.byok_oauth_broker) == 0 && length(google_kms_crypto_key_iam_member.deploy_capability_public_key) == 0 && length(google_kms_crypto_key_iam_member.deploy_tenant_secret_public_key) == 0 && output.broker_service_accounts == {} && output.tenant_secret_broker_service_accounts == {} && output.broker_signing_keys == {} && output.tenant_secret_signing_keys == {}
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
    condition     = length(google_service_account.broker["abcdefghijklmnop"].account_id) == 30 && length(google_service_account.tenant_secret_broker["abcdefghijklmnop"].account_id) <= 30
    error_message = "The 16-character environment boundary must fit GCP's 30-character account ID for both brokers."
  }
}
run "bootstrap_17_character_broker_forbidden" {
  command = plan
  variables { byok_broker_environments = ["abcdefghijklmnopq"] }
  expect_failures = [var.byok_broker_environments]
}
run "overlapping_credential_prefixes_forbidden" {
  command = plan
  variables { byok_broker_environments = ["prod", "prod-cred-blue"] }
  expect_failures = [var.byok_broker_environments]
}
run "overlapping_tenant_secret_prefixes_forbidden" {
  command = plan
  variables { byok_broker_environments = ["test", "test-tsec-blue"] }
  expect_failures = [var.byok_broker_environments]
}
run "tenant_secret_prefix_overlapping_credentials_forbidden" {
  command = plan
  variables { byok_broker_environments = ["test", "test-tsec"] }
  expect_failures = [var.byok_broker_environments]
}
run "distinct_credential_prefixes_with_shared_id_prefix" {
  command = plan
  variables { byok_broker_environments = ["development", "develop"] }
  assert {
    condition     = length(google_service_account.broker) == 2 && length(google_service_account.tenant_secret_broker) == 2
    error_message = "Only namespace-prefix overlap is forbidden; a shared ID prefix is safe."
  }
}
run "unsupported_broker_signing_region_forbidden" {
  command = plan
  variables {
    byok_broker_environments = ["test"]
    broker_signing_region    = "us-west1"
  }
  expect_failures = [var.broker_signing_region]
}

run "production_broker_bootstrap" {
  command   = apply
  state_key = "production-broker-bootstrap"
  variables {
    deployment_tier            = "production"
    nonproduction_project_id   = "aidash-nonproduction-fixture"
    nonproduction_state_bucket = "aidash-nonproduction-state"
    staging_release_bucket     = "aidash-nonproduction-releases"
    byok_broker_environments   = ["production"]
  }
  override_resource {
    target = google_service_account.automation["deploy"]
    values = {
      email = "aidash-deploy@aidash-fixture.iam.gserviceaccount.com"
      name  = "projects/aidash-fixture/serviceAccounts/aidash-deploy@aidash-fixture.iam.gserviceaccount.com"
    }
  }
  override_resource {
    target = google_service_account.broker["production"]
    values = {
      email     = "aidash-production-broker@aidash-fixture.iam.gserviceaccount.com"
      name      = "projects/aidash-fixture/serviceAccounts/aidash-production-broker@aidash-fixture.iam.gserviceaccount.com"
      unique_id = "10000000000000000005"
    }
  }
  override_resource {
    target = google_service_account.tenant_secret_broker["production"]
    values = {
      email     = "aidash-production-tsec@aidash-fixture.iam.gserviceaccount.com"
      name      = "projects/aidash-fixture/serviceAccounts/aidash-production-tsec@aidash-fixture.iam.gserviceaccount.com"
      unique_id = "10000000000000000006"
    }
  }
  override_resource {
    target = google_kms_key_ring.capability["production"]
    values = { id = "projects/aidash-fixture/locations/us-central1/keyRings/aidash-production-capability" }
  }
  override_resource {
    target = google_kms_crypto_key.capability["production"]
    values = { id = "projects/aidash-fixture/locations/us-central1/keyRings/aidash-production-capability/cryptoKeys/capability" }
  }
  override_resource {
    target = google_kms_crypto_key.tenant_secret["production"]
    values = { id = "projects/aidash-fixture/locations/us-central1/keyRings/aidash-production-capability/cryptoKeys/tenant-secret" }
  }
  assert {
    condition = (
      keys(output.broker_service_accounts) == ["production"] &&
      output.broker_signing_keys["production"] == "projects/aidash-fixture/locations/us-central1/keyRings/aidash-production-capability/cryptoKeys/capability" &&
      keys(output.tenant_secret_broker_service_accounts) == ["production"] &&
      output.tenant_secret_signing_keys["production"] == "projects/aidash-fixture/locations/us-central1/keyRings/aidash-production-capability/cryptoKeys/tenant-secret" &&
      google_project_iam_member.byok_broker_read["production"].condition[0].expression == "resource.name.startsWith(\"projects/123456789012/secrets/aidash-production-cred-\")" &&
      google_project_iam_member.byok_tenant_secret_broker_read["production"].condition[0].expression == "resource.name.startsWith(\"projects/123456789012/secrets/aidash-production-tsec-\")"
    )
    error_message = "Production bootstrap owns only its dedicated broker identities and stable keys with production-only prefixes."
  }
}

run "overlapping_oauth_prefixes_forbidden" {
  command = plan
  variables { byok_broker_environments = ["prod", "prod-oauth-blue"] }
  expect_failures = [var.byok_broker_environments]
}
