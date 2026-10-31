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
  deployment_tier              = "production"
  byok_project_id              = "aidash-byok-fixture"
  environment_id               = "production"
  environment_kind             = "production"
  secret_prefix                = "aidash-production-cred-"
  runtime_service_account      = "runtime@aidash-fixture.iam.gserviceaccount.com"
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
}
run "staging" {
  command = apply
  variables {
    deployment_tier              = "nonproduction"
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
    condition     = length(google_cloud_run_v2_service.broker) == 0
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
    condition     = length(google_cloud_run_v2_service.broker) == 0
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
run "mutable_tag_forbidden" {
  command = plan
  variables { image = "us-central1-docker.pkg.dev/aidash-fixture/aidash/broker:latest" }
  expect_failures = [var.image]
}
run "foreign_registry_forbidden" {
  command = plan
  variables { image = "docker.io/library/broker@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" }
  expect_failures = [var.image]
}
run "other_project_registry_forbidden" {
  command = plan
  variables { image = "us-central1-docker.pkg.dev/someone-else/aidash/broker@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" }
  expect_failures = [var.image]
}
run "other_repository_forbidden" {
  command = plan
  variables { image = "us-central1-docker.pkg.dev/aidash-fixture/aidash/app@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" }
  expect_failures = [var.image]
}

run "production_broker_cannot_use_nonproduction_tier" {
  command = plan
  variables {
    deployment_tier = "nonproduction"
    enabled         = true
  }
  expect_failures = [var.enabled]
}

run "tenant_secrets" {
  command = apply
  variables {
    enabled                      = true
    role                         = "tenant-secrets"
    bind_runtime_signer          = false
    secret_prefix                = "aidash-production-tsec-"
    broker_service_account_email = "aidash-production-tsec@aidash-fixture.iam.gserviceaccount.com"
    signing_key_id               = "projects/aidash-fixture/locations/us-central1/keyRings/aidash-production-capability/cryptoKeys/tenant-secret"
    secondary_signing_key_id     = "projects/aidash-fixture/locations/us-central1/keyRings/aidash-production-capability/cryptoKeys/capability"
    secondary_issuer             = "aidash-worker"
    issuer                       = "aidash-server"
  }
  assert {
    condition     = google_cloud_run_v2_service.broker[0].name == "aidash-production-tenant-secret-broker" && google_cloud_run_v2_service.broker[0].ingress == "INGRESS_TRAFFIC_INTERNAL_ONLY" && google_cloud_run_v2_service.broker[0].invoker_iam_disabled
    error_message = "The Tenant Secret broker is its own internal, token-authenticated service."
  }
  assert {
    condition     = google_cloud_run_v2_service.broker[0].template[0].service_account == var.broker_service_account_email
    error_message = "The Tenant Secret broker runs as its own bootstrap service account."
  }
  assert {
    condition = (
      { for item in google_cloud_run_v2_service.broker[0].template[0].containers[0].env : item.name => item.value }["AIDASH_BROKER_ROLE"] == "tenant-secrets" &&
      { for item in google_cloud_run_v2_service.broker[0].template[0].containers[0].env : item.name => item.value }["AIDASH_TENANT_SECRET_PREFIX"] == "aidash-production-tsec-" &&
      { for item in google_cloud_run_v2_service.broker[0].template[0].containers[0].env : item.name => item.value }["AIDASH_SECRET_CALL_AUDIENCE"] == "production-tsec" &&
      !contains(keys({ for item in google_cloud_run_v2_service.broker[0].template[0].containers[0].env : item.name => item.value }), "AIDASH_CAPABILITY_ISSUER")
    )
    error_message = "The Tenant Secret role is selected by fixed environment names later Rust slices read and never shares the provider audience."
  }
  assert {
    condition = (
      jsondecode({ for item in google_cloud_run_v2_service.broker[0].template[0].containers[0].env : item.name => item.value }["AIDASH_SECRET_CALL_ISSUERS"]) == {
        "projects/aidash-fixture/locations/us-central1/keyRings/aidash-production-capability/cryptoKeys/capability/cryptoKeyVersions/1"    = "aidash-worker"
        "projects/aidash-fixture/locations/us-central1/keyRings/aidash-production-capability/cryptoKeys/tenant-secret/cryptoKeyVersions/1" = "aidash-server"
      } &&
      keys(jsondecode({ for item in google_cloud_run_v2_service.broker[0].template[0].containers[0].env : item.name => item.value }["AIDASH_CAPABILITY_PUBLIC_KEYS"])) == keys(jsondecode({ for item in google_cloud_run_v2_service.broker[0].template[0].containers[0].env : item.name => item.value }["AIDASH_SECRET_CALL_ISSUERS"]))
    )
    error_message = "Each verification key maps to the issuer its tokens must carry, over exactly the public-key set."
  }
  assert {
    condition = keys(jsondecode({ for item in google_cloud_run_v2_service.broker[0].template[0].containers[0].env : item.name => item.value }["AIDASH_CAPABILITY_PUBLIC_KEYS"])) == [
      "projects/aidash-fixture/locations/us-central1/keyRings/aidash-production-capability/cryptoKeys/capability/cryptoKeyVersions/1",
      "projects/aidash-fixture/locations/us-central1/keyRings/aidash-production-capability/cryptoKeys/tenant-secret/cryptoKeyVersions/1",
    ]
    error_message = "The Tenant Secret broker verifies both the worker and server keys."
  }
  assert {
    condition     = output.server_configuration.kid == "projects/aidash-fixture/locations/us-central1/keyRings/aidash-production-capability/cryptoKeys/tenant-secret/cryptoKeyVersions/1" && output.server_configuration.issuer == "aidash-server" && output.server_configuration.audience == "production-tsec" && output.server_configuration.endpoint == "https://broker-fixture.run.app" && output.worker_configuration == null
    error_message = "The server configuration exports the bare Cloud Run URI and the Tenant Secret audience."
  }
}
run "tenant_role_rejects_key_material_prefix" {
  command = plan
  variables {
    enabled                      = true
    role                         = "tenant-secrets"
    secret_prefix                = "aidash-production-cred-"
    broker_service_account_email = "aidash-production-tsec@aidash-fixture.iam.gserviceaccount.com"
    signing_key_id               = "projects/aidash-fixture/locations/us-central1/keyRings/aidash-production-capability/cryptoKeys/tenant-secret"
    secondary_signing_key_id     = "projects/aidash-fixture/locations/us-central1/keyRings/aidash-production-capability/cryptoKeys/capability"
    secondary_issuer             = "aidash-worker"
  }
  expect_failures = [var.secret_prefix]
}
run "key_material_role_rejects_tenant_prefix" {
  command = plan
  variables {
    enabled       = true
    secret_prefix = "aidash-production-tsec-"
  }
  expect_failures = [var.secret_prefix]
}
run "tenant_role_requires_the_worker_verification_key" {
  command = plan
  variables {
    enabled                      = true
    role                         = "tenant-secrets"
    secret_prefix                = "aidash-production-tsec-"
    broker_service_account_email = "aidash-production-tsec@aidash-fixture.iam.gserviceaccount.com"
    signing_key_id               = "projects/aidash-fixture/locations/us-central1/keyRings/aidash-production-capability/cryptoKeys/tenant-secret"
    secondary_issuer             = "aidash-worker"
  }
  expect_failures = [var.secondary_signing_key_id]
}
run "tenant_role_requires_the_worker_issuer" {
  command = plan
  variables {
    enabled                      = true
    role                         = "tenant-secrets"
    secret_prefix                = "aidash-production-tsec-"
    broker_service_account_email = "aidash-production-tsec@aidash-fixture.iam.gserviceaccount.com"
    signing_key_id               = "projects/aidash-fixture/locations/us-central1/keyRings/aidash-production-capability/cryptoKeys/tenant-secret"
    secondary_signing_key_id     = "projects/aidash-fixture/locations/us-central1/keyRings/aidash-production-capability/cryptoKeys/capability"
  }
  expect_failures = [var.secondary_issuer]
}
run "key_material_role_rejects_a_secondary_key" {
  command = plan
  variables {
    enabled                  = true
    secondary_signing_key_id = "projects/aidash-fixture/locations/us-central1/keyRings/aidash-production-capability/cryptoKeys/capability"
  }
  expect_failures = [var.secondary_signing_key_id]
}
run "key_material_role_rejects_a_secondary_issuer" {
  command = plan
  variables {
    enabled          = true
    secondary_issuer = "aidash-worker"
  }
  expect_failures = [var.secondary_issuer]
}
