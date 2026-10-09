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
  command   = apply
  state_key = "broker-environments"
  variables { byok_broker_environments = ["production", "test"] }
  override_resource {
    target = google_service_account.automation["deploy"]
    values = {
      email = "aidash-deploy@aidash-fixture.iam.gserviceaccount.com"
      name  = "projects/aidash-fixture/serviceAccounts/aidash-deploy@aidash-fixture.iam.gserviceaccount.com"
    }
  }
  override_resource {
    target = google_service_account.broker["production"]
    values = { email = "aidash-production-broker@aidash-fixture.iam.gserviceaccount.com" }
  }
  override_resource {
    target = google_service_account.broker["test"]
    values = { email = "aidash-test-broker@aidash-fixture.iam.gserviceaccount.com" }
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
    target = google_kms_key_ring.capability["test"]
    values = { id = "projects/aidash-fixture/locations/us-central1/keyRings/aidash-test-capability" }
  }
  override_resource {
    target = google_kms_crypto_key.capability["test"]
    values = { id = "projects/aidash-fixture/locations/us-central1/keyRings/aidash-test-capability/cryptoKeys/capability" }
  }
  assert {
    condition     = alltrue([for id, account in google_service_account.broker : account.project == var.project_id && account.account_id == "aidash-${id}-broker" && google_project_iam_member.byok_broker_read[id].project == var.byok_project_id && google_project_iam_member.byok_broker_read[id].role == google_project_iam_custom_role.byok_broker_read[0].name && google_project_iam_member.byok_broker_read[id].member == "serviceAccount:${account.email}" && google_project_iam_member.byok_broker_read[id].condition[0].expression == "resource.name.startsWith(\"projects/123456789012/secrets/aidash-${id}-cred-\")"])
    error_message = "Only dedicated broker SAs may receive prefix-conditioned BYOK read access."
  }
  assert {
    condition     = output.broker_service_accounts == { production = "aidash-production-broker@aidash-fixture.iam.gserviceaccount.com", test = "aidash-test-broker@aidash-fixture.iam.gserviceaccount.com" }
    error_message = "Export the per-environment broker identities for deployment."
  }
  assert {
    condition     = alltrue([for id, key in google_kms_crypto_key.capability : key.purpose == "ASYMMETRIC_SIGN" && key.name == "capability" && key.key_ring == google_kms_key_ring.capability[id].id && key.version_template[0].algorithm == "EC_SIGN_ED25519" && google_kms_key_ring.capability[id].name == "aidash-${id}-capability" && google_kms_key_ring.capability[id].project == var.project_id]) && keys(output.broker_signing_keys) == ["production", "test"]
    error_message = "Bootstrap alone owns and exports the permanent per-environment Ed25519 signing keys."
  }
  assert {
    condition     = keys(google_kms_crypto_key_iam_member.deploy_capability_public_key) == ["production", "test"] && alltrue([for id, grant in google_kms_crypto_key_iam_member.deploy_capability_public_key : grant.crypto_key_id == google_kms_crypto_key.capability[id].id && grant.role == "roles/cloudkms.publicKeyViewer" && grant.member == "serviceAccount:aidash-deploy@aidash-fixture.iam.gserviceaccount.com"])
    error_message = "Bootstrap grants deploy only key-scoped public-key reads on each permanent signing key."
  }
}
run "brokers_opt_in_only" {
  command = apply
  assert {
    condition     = length(google_service_account.broker) == 0 && length(google_project_iam_member.byok_broker_read) == 0 && length(google_kms_crypto_key.capability) == 0 && length(google_kms_crypto_key_iam_member.deploy_capability_public_key) == 0
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
    condition     = length(google_service_account.broker) == 0 && length(google_project_iam_member.byok_broker_read) == 0 && length(google_kms_crypto_key_iam_member.deploy_capability_public_key) == 0 && output.broker_service_accounts == {} && output.broker_signing_keys == {}
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
run "overlapping_credential_prefixes_forbidden" {
  command = plan
  variables { byok_broker_environments = ["prod", "prod-cred-blue"] }
  expect_failures = [var.byok_broker_environments]
}
run "distinct_credential_prefixes_with_shared_id_prefix" {
  command = plan
  variables { byok_broker_environments = ["prod", "production"] }
  assert {
    condition     = length(google_service_account.broker) == 2
    error_message = "Only credential-prefix overlap is forbidden; a shared ID prefix is safe."
  }
}
