mock_provider "google" {
  mock_resource "google_kms_key_ring" {
    defaults = { id = "projects/aidash-fixture/locations/us-central1/keyRings/aidash-test-capability" }
  }
  mock_resource "google_kms_crypto_key" {
    defaults = { id = "projects/aidash-fixture/locations/us-central1/keyRings/aidash-test-capability/cryptoKeys/capability" }
  }
  mock_resource "google_cloud_run_v2_service" { defaults = { uri = "https://broker.run.app" } }
  mock_resource "google_service_account" {
    defaults = {
      name  = "projects/aidash-fixture/serviceAccounts/runtime@aidash-fixture.iam.gserviceaccount.com"
      email = "runtime@aidash-fixture.iam.gserviceaccount.com"
    }
  }
  mock_data "google_project" { defaults = { number = "123456789012" } }
  mock_data "google_kms_crypto_key_version" {
    defaults = { public_key = [{ pem = "fixture-public-key", algorithm = "EC_SIGN_ED25519" }] }
  }
}
mock_provider "cloudflare" {}
variables {
  project_id             = "aidash-fixture"
  byok_project_id        = "aidash-byok-fixture"
  broker_image_lock_file = "tests/fixtures/broker-image.json"
}

run "bootstrap_keys" {
  command   = apply
  state_key = "permanent-bootstrap"
  module { source = "../bootstrap" }
  variables {
    state_bucket_name        = "aidash-fixture-state"
    release_bucket_name      = "aidash-fixture-releases"
    notification_bucket_name = "aidash-fixture-notifications"
    byok_broker_environments = ["test"]
    security_alert_emails    = ["security@example.test"]
  }
  override_resource {
    target = google_service_account.broker["test"]
    values = {
      email = "aidash-test-broker@aidash-fixture.iam.gserviceaccount.com"
      name  = "projects/aidash-fixture/serviceAccounts/aidash-test-broker@aidash-fixture.iam.gserviceaccount.com"
    }
  }
  override_resource {
    target = google_service_account.tenant_secret_broker["test"]
    values = {
      email = "aidash-test-tsec@aidash-fixture.iam.gserviceaccount.com"
      name  = "projects/aidash-fixture/serviceAccounts/aidash-test-tsec@aidash-fixture.iam.gserviceaccount.com"
    }
  }
  override_resource {
    target = google_kms_crypto_key.tenant_secret["test"]
    values = { id = "projects/aidash-fixture/locations/us-central1/keyRings/aidash-test-capability/cryptoKeys/tenant-secret" }
  }
  assert {
    condition     = output.broker_signing_keys["test"] == "projects/aidash-fixture/locations/us-central1/keyRings/aidash-test-capability/cryptoKeys/capability" && output.tenant_secret_signing_keys["test"] == "projects/aidash-fixture/locations/us-central1/keyRings/aidash-test-capability/cryptoKeys/tenant-secret"
    error_message = "Bootstrap exports the stable worker and server signing-key resources, independently of Cloud Run enablement."
  }
}

run "environment_enabled" {
  command   = apply
  state_key = "environment-lifecycle"
  variables {
    deploy_service_account = "deploy@aidash-fixture.iam.gserviceaccount.com"
    cloudflare_zone_id     = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    environments = { test = {
      kind        = "test"
      incarnation = "aaaaaaaaaaaa"
      generation  = 1
      running     = false
      published   = false
      spot        = true
      release_sha = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    } }
    credential_brokers = { test = {
      enabled                                    = true
      byok_project_id                            = "aidash-byok-fixture"
      secret_prefix                              = "aidash-test-cred-"
      broker_service_account_email               = run.bootstrap_keys.broker_service_accounts["test"]
      signing_key_id                             = run.bootstrap_keys.broker_signing_keys["test"]
      tenant_secret_broker_service_account_email = run.bootstrap_keys.tenant_secret_broker_service_accounts["test"]
      tenant_secret_signing_key_id               = run.bootstrap_keys.tenant_secret_signing_keys["test"]
    } }
  }
  assert {
    condition     = output.credential_brokers["test"].kid == "${run.bootstrap_keys.broker_signing_keys["test"]}/cryptoKeyVersions/1" && output.tenant_secret_brokers["test"].kid == "${run.bootstrap_keys.tenant_secret_signing_keys["test"]}/cryptoKeyVersions/1"
    error_message = "Worker and server capability authority use their own bootstrap keys."
  }
}

run "broker_rotation_plan" {
  command   = plan
  state_key = "environment-lifecycle"
  variables {
    deploy_service_account = "deploy@aidash-fixture.iam.gserviceaccount.com"
    cloudflare_zone_id     = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    environments           = run.environment_enabled.managed_configuration
    credential_brokers = { test = merge(run.environment_enabled.managed_credential_brokers["test"], {
      signing_version       = "2"
      verification_versions = ["1", "2"]
    }) }
  }
  override_data {
    target = module.credential_broker["test"].data.google_kms_crypto_key_version.verification["2"]
    values = { public_key = [{ pem = "fixture-public-key-2", algorithm = "EC_SIGN_ED25519" }] }
  }
  override_data {
    target = module.tenant_secret_broker["test"].data.google_kms_crypto_key_version.secondary_verification["2"]
    values = { public_key = [{ pem = "fixture-public-key-2", algorithm = "EC_SIGN_ED25519" }] }
  }
  assert {
    condition     = output.credential_brokers["test"].kid == "${run.bootstrap_keys.broker_signing_keys["test"]}/cryptoKeyVersions/2" && output.tenant_secret_brokers["test"].kid == "${run.bootstrap_keys.tenant_secret_signing_keys["test"]}/cryptoKeyVersions/1" && google_kms_crypto_key_iam_member.tenant_secret_signer["test"].crypto_key_id == run.bootstrap_keys.tenant_secret_signing_keys["test"]
    error_message = "Managed workers sign with the explicit version; the server key and its signer stay separate."
  }

}

run "broker_disabled_plan" {
  command   = plan
  state_key = "environment-lifecycle"
  variables {
    deploy_service_account = "deploy@aidash-fixture.iam.gserviceaccount.com"
    cloudflare_zone_id     = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    environments           = run.environment_enabled.managed_configuration
    credential_brokers     = { test = merge(run.environment_enabled.managed_credential_brokers["test"], { enabled = false }) }
  }
  assert {
    condition     = length(module.credential_broker) == 0 && length(module.tenant_secret_broker) == 0 && length(google_kms_crypto_key_iam_member.broker_signer) == 0 && length(google_kms_crypto_key_iam_member.tenant_secret_signer) == 0
    error_message = "Disable removes both services and both signer bindings."
  }
}
run "broker_disabled_apply" {
  command   = apply
  state_key = "environment-lifecycle"
  variables {
    deploy_service_account = "deploy@aidash-fixture.iam.gserviceaccount.com"
    cloudflare_zone_id     = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    environments           = run.environment_enabled.managed_configuration
    credential_brokers     = { test = merge(run.environment_enabled.managed_credential_brokers["test"], { enabled = false }) }
  }
}
run "broker_reenabled_plan" {
  command   = plan
  state_key = "environment-lifecycle"
  variables {
    deploy_service_account = "deploy@aidash-fixture.iam.gserviceaccount.com"
    cloudflare_zone_id     = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    environments           = run.environment_enabled.managed_configuration
    credential_brokers     = run.environment_enabled.managed_credential_brokers
  }
  assert {
    condition     = module.credential_broker["test"].signing_key == run.bootstrap_keys.broker_signing_keys["test"] && module.tenant_secret_broker["test"].signing_key == run.bootstrap_keys.tenant_secret_signing_keys["test"]
    error_message = "Re-enable consumes the original keys instead of recreating their immutable names."
  }
}
run "broker_reenabled_apply" {
  command   = apply
  state_key = "environment-lifecycle"
  variables {
    deploy_service_account = "deploy@aidash-fixture.iam.gserviceaccount.com"
    cloudflare_zone_id     = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    environments           = run.environment_enabled.managed_configuration
    credential_brokers     = run.environment_enabled.managed_credential_brokers
  }
}
run "environment_removed_plan" {
  command   = plan
  state_key = "environment-lifecycle"
  variables {
    deploy_service_account = "deploy@aidash-fixture.iam.gserviceaccount.com"
    cloudflare_zone_id     = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    environments           = {}
    credential_brokers     = {}
  }
  assert {
    condition     = length(module.credential_broker) == 0 && length(module.tenant_secret_broker) == 0 && length(google_kms_crypto_key_iam_member.broker_signer) == 0 && length(google_kms_crypto_key_iam_member.tenant_secret_signer) == 0
    error_message = "Environment removal cannot own signing-key destruction."
  }
}
run "bootstrap_after_retirement_plan" {
  command   = plan
  state_key = "permanent-bootstrap"
  module { source = "../bootstrap" }
  variables {
    state_bucket_name        = "aidash-fixture-state"
    release_bucket_name      = "aidash-fixture-releases"
    notification_bucket_name = "aidash-fixture-notifications"
    byok_broker_environments = ["test"]
    security_alert_emails    = ["security@example.test"]
  }
  assert {
    condition     = output.broker_signing_keys == run.bootstrap_keys.broker_signing_keys && output.tenant_secret_signing_keys == run.bootstrap_keys.tenant_secret_signing_keys
    error_message = "Automation retirement leaves permanent bootstrap key state unchanged."
  }
}
