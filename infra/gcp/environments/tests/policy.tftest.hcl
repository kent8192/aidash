mock_provider "google" {
  mock_resource "google_cloud_run_v2_service" { defaults = { uri = "https://broker.run.app" } }
  mock_resource "google_service_account" {
    defaults = {
      name  = "projects/aidash-fixture/serviceAccounts/runtime@aidash-fixture.iam.gserviceaccount.com"
      email = "runtime@aidash-fixture.iam.gserviceaccount.com"
    }
  }
  mock_data "google_kms_crypto_key_version" {
    defaults = { public_key = [{ pem = "fixture-public-key", algorithm = "EC_SIGN_ED25519" }] }
  }
}
mock_provider "cloudflare" {}

variables {
  project_id             = "aidash-fixture"
  byok_project_id        = "aidash-byok-fixture"
  release_bucket         = "aidash-fixture-releases"
  deploy_service_account = "deploy@aidash-fixture.iam.gserviceaccount.com"
  cloudflare_zone_id     = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
}

run "empty_has_no_hosts_or_dns" {
  command = plan
  assert {
    condition     = length(module.environment) == 0 && length(cloudflare_dns_record.environment) == 0
    error_message = "An unrequested environment must not be provisioned."
  }
  assert {
    condition     = length(module.credential_broker) == 0
    error_message = "Brokers require explicit Provider Credential enablement."
  }
  assert {
    condition     = google_compute_disk.preview_tls.name == "aidash-preview-tls" && google_compute_disk.preview_tls.size == 10
    error_message = "The preview TLS store must survive even when every PR is retired."
  }
}

run "no_broker_in_preview" {
  command = plan
  variables {
    environments = {
      pr-137 = {
        kind          = "pr"
        incarnation   = "aaaaaaaaaaaa"
        generation    = 1
        running       = false
        published     = false
        spot          = true
        bundle_object = "bundles/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.tar.gz"
        bundle_sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        release_sha   = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
      }
    }
    credential_brokers = {
      pr-137 = {
        enabled                      = true
        byok_project_id              = "aidash-byok-fixture"
        secret_prefix                = "aidash-pr-137-cred-"
        broker_service_account_email = "aidash-pr-137-broker@aidash-fixture.iam.gserviceaccount.com"
        image                        = "broker@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
      }
    }
  }
  expect_failures = [var.credential_brokers]
}

run "staging_uses_bootstrap_broker_sa" {
  command = apply
  variables {
    environments = {
      test = {
        kind          = "test"
        incarnation   = "aaaaaaaaaaaa"
        generation    = 1
        running       = false
        published     = false
        spot          = true
        bundle_object = "bundles/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.tar.gz"
        bundle_sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        release_sha   = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
      }
    }
    credential_brokers = {
      test = {
        enabled                      = true
        byok_project_id              = "aidash-byok-fixture"
        secret_prefix                = "aidash-test-cred-"
        broker_service_account_email = "aidash-test-broker@aidash-fixture.iam.gserviceaccount.com"
        image                        = "broker@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
      }
    }
  }
  assert {
    condition     = module.credential_broker["test"].service_account == var.credential_brokers["test"].broker_service_account_email
    error_message = "Environment composition must consume the broker SA from bootstrap."
  }
  assert {
    condition     = google_kms_crypto_key_iam_member.broker_signer["test"].member == "serviceAccount:${module.environment["test"].runtime_service_account}" && google_kms_crypto_key_iam_member.broker_signer["test"].role == "roles/cloudkms.signer" && google_kms_crypto_key_iam_member.broker_signer["test"].crypto_key_id == module.credential_broker["test"].signing_key
    error_message = "Only the matching VM runtime may sign capabilities for its broker."
  }
  assert {
    condition     = output.environments["test"].provider_credentials.broker == module.credential_broker["test"].worker_configuration && output.environments["test"].provider_credentials.store.byok_project_id == var.byok_project_id && output.environments["test"].provider_credentials.store.environment_id == "test" && output.environments["test"].provider_credentials.broker.endpoint == "https://broker.run.app/api/v1"
    error_message = "Managed VM output must carry the actual Store and broker worker settings."
  }
}

run "broker_must_use_the_environment_store_project" {
  command = plan
  variables {
    environments = { test = {
      kind          = "test"
      incarnation   = "aaaaaaaaaaaa"
      generation    = 1
      running       = false
      published     = false
      spot          = true
      bundle_object = "bundles/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.tar.gz"
      bundle_sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
      release_sha   = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    } }
    credential_brokers = {
      test = {
        enabled                      = true
        byok_project_id              = "aidash-byok-other"
        secret_prefix                = "aidash-test-cred-"
        broker_service_account_email = "aidash-test-broker@aidash-fixture.iam.gserviceaccount.com"
        image                        = "broker@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
      }
    }
  }
  expect_failures = [var.credential_brokers]
}

run "stopped_retains_host_and_disks_without_dns" {
  command = plan
  variables {
    environments = {
      test = {
        kind          = "test"
        incarnation   = "aaaaaaaaaaaa"
        generation    = 1
        running       = false
        published     = false
        spot          = true
        bundle_object = "bundles/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.tar.gz"
        bundle_sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        release_sha   = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
      }
    }
  }
  assert {
    condition     = length(module.environment) == 1 && length(cloudflare_dns_record.environment) == 0
    error_message = "Stopping must retain owned resources while withdrawing DNS."
  }
}

run "no_production" {
  command = plan
  variables {
    environments = {
      production = {
        kind          = "production"
        incarnation   = "aaaaaaaaaaaa"
        generation    = 1
        running       = true
        published     = false
        spot          = false
        bundle_object = "bundles/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.tar.gz"
        bundle_sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        release_sha   = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
      }
    }
  }
  expect_failures = [var.environments]
}

run "no_two_running_previews" {
  command = plan
  variables {
    environments = { for id in ["pr-1", "pr-2"] : id => {
      kind          = "pr"
      incarnation   = "aaaaaaaaaaaa"
      generation    = 1
      running       = true
      published     = false
      spot          = true
      bundle_object = "bundles/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.tar.gz"
      bundle_sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
      release_sha   = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    } }
  }
  expect_failures = [var.environments]
}
