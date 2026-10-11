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
  project_id      = "aidash-fixture"
  byok_project_id = "aidash-byok-fixture"
}

run "bootstrap_keys" {
  command   = apply
  state_key = "permanent-bootstrap"
  module { source = "../bootstrap" }
  variables {
    state_bucket_name        = "aidash-fixture-state"
    release_bucket_name      = "aidash-fixture-releases"
    byok_broker_environments = ["test"]
  }
  override_resource {
    target = google_service_account.broker["test"]
    values = { email = "aidash-test-broker@aidash-fixture.iam.gserviceaccount.com" }
  }
  assert {
    condition     = output.broker_signing_keys["test"] == "projects/aidash-fixture/locations/us-central1/keyRings/aidash-test-capability/cryptoKeys/capability"
    error_message = "Bootstrap exports the stable signing-key resource, independently of Cloud Run enablement."
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
      enabled                      = true
      byok_project_id              = "aidash-byok-fixture"
      secret_prefix                = "aidash-test-cred-"
      broker_service_account_email = run.bootstrap_keys.broker_service_accounts["test"]
      signing_key_id               = run.bootstrap_keys.broker_signing_keys["test"]
      image                        = "broker@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    } }
  }
  assert {
    condition     = output.credential_brokers["test"].kid == "${run.bootstrap_keys.broker_signing_keys["test"]}/cryptoKeyVersions/1"
    error_message = "Worker capability authority uses the bootstrap key."
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
  assert {
    condition     = output.credential_brokers["test"].kid == "${run.bootstrap_keys.broker_signing_keys["test"]}/cryptoKeyVersions/2"
    error_message = "Managed workers must sign with the operator-selected numeric version."
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
    condition     = length(module.credential_broker) == 0 && length(google_kms_crypto_key_iam_member.broker_signer) == 0
    error_message = "Disable removes only the service and worker signer binding."
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
    condition     = module.credential_broker["test"].signing_key == run.bootstrap_keys.broker_signing_keys["test"]
    error_message = "Re-enable consumes the original key instead of recreating its immutable name."
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
    condition     = length(module.credential_broker) == 0 && length(google_kms_crypto_key_iam_member.broker_signer) == 0
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
    byok_broker_environments = ["test"]
  }
  assert {
    condition     = output.broker_signing_keys == run.bootstrap_keys.broker_signing_keys
    error_message = "Automation retirement leaves permanent bootstrap key state unchanged."
  }
}
