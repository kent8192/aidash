mock_provider "google" {
  mock_resource "google_cloud_run_v2_service" {
    defaults = { uri = "https://broker-fixture.run.app" }
  }
  mock_data "google_project" { defaults = { number = "123456789" } }
  mock_data "google_kms_crypto_key_version" {
    defaults = { public_key = [{ pem = "PUBLIC-KEY-FIXTURE", algorithm = "EC_SIGN_ED25519" }] }
  }
}
variables {
  project_id                   = "aidash-fixture"
  byok_project_id              = "aidash-byok-fixture"
  environment_id               = "production"
  environment_kind             = "production"
  secret_prefix                = "aidash-production-cred-"
  runtime_service_account      = "runtime@aidash-fixture.iam.gserviceaccount.com"
  deploy_service_account       = "deploy@aidash-fixture.iam.gserviceaccount.com"
  broker_service_account_email = "aidash-production-broker@aidash-fixture.iam.gserviceaccount.com"
  signing_key_id               = "projects/aidash-fixture/locations/us-central1/keyRings/aidash-production-capability/cryptoKeys/capability"
  image                        = "us-central1-docker.pkg.dev/aidash-fixture/aidash/broker@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
}
run "seventeen_character_environment_forbidden" {
  command = plan
  variables {
    environment_id               = "abcdefghijklmnopq"
    secret_prefix                = "aidash-abcdefghijklmnopq-cred-"
    broker_service_account_email = "aidash-abcdefghijklmnopq-broker@aidash-fixture.iam.gserviceaccount.com"
  }
  expect_failures = [var.environment_id]
}
run "production" {
  command = apply
  variables { enabled = true }
  assert {
    condition     = google_cloud_run_v2_service.broker[0].ingress == "INGRESS_TRAFFIC_INTERNAL_ONLY" && google_cloud_run_v2_service.broker[0].invoker_iam_disabled
    error_message = "Only Capability Tokens may authenticate internal broker ingress."
  }
  assert {
    condition     = google_cloud_run_v2_service.broker[0].template[0].scaling[0].min_instance_count == 1 && google_cloud_run_v2_service.broker[0].template[0].scaling[0].max_instance_count == 10 && google_cloud_run_v2_service.broker[0].template[0].timeout == "3600s"
    error_message = "Production broker defaults must match the admission/streaming contract."
  }
  assert {
    condition     = one([for value in google_cloud_run_v2_service.broker[0].template[0].containers[0].env : value.value if value.name == "AIDASH_BROKER_TIMEOUT_SECS"]) == "3570"
    error_message = "The broker must finish 30 seconds before the platform deadline."
  }
  assert {
    condition     = google_cloud_run_v2_service.broker[0].template[0].service_account == var.broker_service_account_email && google_kms_crypto_key_iam_member.signer[0].role == "roles/cloudkms.signer" && google_kms_crypto_key_iam_member.signer[0].member == "serviceAccount:${var.runtime_service_account}"
    error_message = "Run as the bootstrap broker SA; the runtime can only sign."
  }
  assert {
    condition     = google_cloud_run_v2_service.broker[0].template[0].max_instance_request_concurrency == 4 && google_cloud_run_v2_service.broker[0].template[0].containers[0].resources[0].limits["memory"] == "1Gi"
    error_message = "Media buffering requires bounded per-instance concurrency and explicit memory."
  }
  assert {
    condition     = google_service_account_iam_member.deploy[0].service_account_id == "projects/${var.project_id}/serviceAccounts/${var.broker_service_account_email}" && google_service_account_iam_member.deploy[0].role == "roles/iam.serviceAccountUser" && google_service_account_iam_member.deploy[0].member == "serviceAccount:${var.deploy_service_account}"
    error_message = "Deploy automation may act as the supplied application-project SA."
  }
}
run "staging" {
  command = apply
  variables {
    enabled                      = true
    environment_id               = "test"
    environment_kind             = "test"
    secret_prefix                = "aidash-test-cred-"
    broker_service_account_email = "aidash-test-broker@aidash-fixture.iam.gserviceaccount.com"
    signing_key_id               = "projects/aidash-fixture/locations/us-central1/keyRings/aidash-test-capability/cryptoKeys/capability"
  }
  assert {
    condition     = google_cloud_run_v2_service.broker[0].template[0].scaling[0].min_instance_count == 0 && google_cloud_run_v2_service.broker[0].template[0].service_account == var.broker_service_account_email
    error_message = "Nonproduction uses its bootstrap broker SA and may scale to zero."
  }
}
run "no_broker_deadline_headroom_forbidden" {
  command = plan
  variables { timeout_secs = 30 }
  expect_failures = [var.timeout_secs]
}
run "minimum_platform_timeout" {
  command = apply
  variables {
    enabled      = true
    timeout_secs = 31
  }
  assert {
    condition     = google_cloud_run_v2_service.broker[0].template[0].timeout == "31s" && one([for value in google_cloud_run_v2_service.broker[0].template[0].containers[0].env : value.value if value.name == "AIDASH_BROKER_TIMEOUT_SECS"]) == "1"
    error_message = "The minimum platform timeout must retain a positive broker deadline and 30 seconds of headroom."
  }
}
run "disabled" {
  command = apply
  assert {
    condition     = length(google_cloud_run_v2_service.broker) == 0 && length(google_service_account_iam_member.deploy) == 0
    error_message = "Without Provider Credentials there must be no broker resources."
  }
}
run "pr" {
  command = apply
  variables {
    environment_id               = "pr-137"
    environment_kind             = "pr"
    secret_prefix                = "aidash-pr-137-cred-"
    broker_service_account_email = "aidash-pr-137-broker@aidash-fixture.iam.gserviceaccount.com"
  }
  assert {
    condition     = length(google_cloud_run_v2_service.broker) == 0 && length(google_service_account_iam_member.deploy) == 0
    error_message = "PR previews must have no broker or Key Material accessor."
  }
}
run "enabled_pr_forbidden" {
  command = plan
  variables {
    enabled                      = true
    environment_id               = "pr-137"
    environment_kind             = "pr"
    secret_prefix                = "aidash-pr-137-cred-"
    broker_service_account_email = "aidash-pr-137-broker@aidash-fixture.iam.gserviceaccount.com"
    signing_key_id               = "projects/aidash-fixture/locations/us-central1/keyRings/aidash-pr-137-capability/cryptoKeys/capability"
  }
  expect_failures = [var.enabled]
}
run "cross_environment_broker_sa_forbidden" {
  command = plan
  variables {
    enabled                      = true
    broker_service_account_email = "aidash-test-broker@aidash-fixture.iam.gserviceaccount.com"
  }
  expect_failures = [var.broker_service_account_email]
}
run "cross_project_broker_sa_forbidden" {
  command = plan
  variables {
    enabled                      = true
    broker_service_account_email = "aidash-production-broker@aidash-byok-fixture.iam.gserviceaccount.com"
  }
  expect_failures = [var.broker_service_account_email]
}
run "insufficient_media_burst_one_forbidden" {
  command = plan
  variables { burst = 1 }
  expect_failures = [var.burst]
}
run "insufficient_media_burst_two_forbidden" {
  command = plan
  variables { burst = 2 }
  expect_failures = [var.burst]
}
run "minimum_media_burst" {
  command = apply
  variables {
    enabled = true
    burst   = 3
  }
  assert {
    condition     = one([for value in google_cloud_run_v2_service.broker[0].template[0].containers[0].env : value.value if value.name == "AIDASH_BROKER_BURST"]) == "3"
    error_message = "The minimum supported media burst must reach the deployed broker configuration."
  }
}
run "cross_environment_signing_key_forbidden" {
  command = plan
  variables {
    enabled        = true
    signing_key_id = "projects/aidash-fixture/locations/us-central1/keyRings/aidash-test-capability/cryptoKeys/capability"
  }
  expect_failures = [var.signing_key_id]
}
