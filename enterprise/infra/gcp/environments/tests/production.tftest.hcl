# SPDX-FileCopyrightText: 2026 kent8192
# SPDX-License-Identifier: BUSL-1.1
# See enterprise/LICENSE.

mock_provider "google" {
  mock_resource "google_cloud_run_v2_service" { defaults = { uri = "https://production-broker.run.app" } }
  mock_resource "google_service_account" {
    defaults = {
      name  = "projects/aidash-production-fixture/serviceAccounts/runtime@aidash-production-fixture.iam.gserviceaccount.com"
      email = "runtime@aidash-production-fixture.iam.gserviceaccount.com"
    }
  }
  mock_data "google_service_account" {
    defaults = {
      name  = "projects/aidash-production-fixture/serviceAccounts/aidash-gke-nodes@aidash-production-fixture.iam.gserviceaccount.com"
      email = "aidash-gke-nodes@aidash-production-fixture.iam.gserviceaccount.com"
    }
  }
  mock_data "google_project" { defaults = { number = "123456789012" } }
  mock_data "google_kms_crypto_key_version" {
    defaults = { public_key = [{ pem = "fixture-public-key", algorithm = "EC_SIGN_ED25519" }] }
  }
}
mock_provider "cloudflare" {}
variables {
  project_id                 = "aidash-production-fixture"
  byok_project_id            = "aidash-production-byok-fixture"
  deployment_tier            = "production"
  staging_project_id         = "aidash-nonproduction-fixture"
  nonproduction_project_id   = "aidash-nonproduction-fixture"
  state_bucket               = "aidash-production-state"
  nonproduction_state_bucket = "aidash-nonproduction-state"
  deploy_service_account     = "deploy@aidash-production-fixture.iam.gserviceaccount.com"
  cloudflare_zone_id         = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
  environments = { production = {
    kind        = "production"
    incarnation = "aaaaaaaaaaaa"
    generation  = 1
    running     = true
    published   = true
    spot        = false
    address     = "203.0.113.10"
  } }
}
run "isolated_regional_production" {
  command = plan
  assert {
    condition = (
      keys(module.environment) == ["production"] &&
      google_container_cluster.aidash.location == "us-central1" &&
      google_container_cluster.aidash.node_locations == toset(["us-central1-a"]) &&
      google_container_cluster.aidash.workload_identity_config[0].workload_pool == "aidash-production-fixture.svc.id.goog" &&
      length(google_service_account.nodes) == 0 &&
      google_container_node_pool.system.node_config[0].service_account == "aidash-gke-nodes@aidash-production-fixture.iam.gserviceaccount.com" &&
      google_container_node_pool.system.node_count == 1 &&
      google_container_node_pool.system.node_config[0].resource_labels.lifecycle == "production" &&
      output.environments.production.hostname == "aidash.run" &&
      cloudflare_dns_record.environment["production"].name == "aidash.run" &&
      output.preview_tls_volume_handle == "projects/aidash-production-fixture/zones/us-central1-a/disks/aidash-preview-tls"
    )
    error_message = "Production uses a separate regional control plane, normal persistent nodes, fixed host, and its own retained disk."
  }
  assert {
    condition = (
      google_container_cluster.aidash.addons_config[0].stateful_ha_config[0].enabled &&
      output.cluster.zones == ["us-central1-a", "us-central1-b"] &&
      length(google_container_node_pool.system_standby) == 1 &&
      google_container_node_pool.system_standby[0].node_locations == toset(["us-central1-b"]) &&
      google_container_node_pool.system_standby[0].autoscaling[0].total_min_node_count == 0 &&
      google_container_node_pool.system_standby[0].autoscaling[0].total_max_node_count == 1 &&
      module.environment["production"].standby_node_pools["us-central1-b"].location == "us-central1-b" &&
      module.environment["production"].standby_node_pools["us-central1-b"].min == 0 &&
      module.environment["production"].standby_node_pools["us-central1-b"].max == 1 &&
      module.environment["production"].standby_node_pools["us-central1-b"].taint == "production"
    )
    error_message = "Production fails over to us-central1-b: Stateful HA, regional disk zones, and standby pools that start at zero nodes (ADR 0067)."
  }
}
run "production_broker_uses_production_identity" {
  command = plan
  variables {
    broker_image_lock_file = "tests/fixtures/broker-image.json"
    credential_brokers = { production = {
      enabled                      = true
      byok_project_id              = "aidash-production-byok-fixture"
      secret_prefix                = "aidash-production-cred-"
      broker_service_account_email = "aidash-production-broker@aidash-production-fixture.iam.gserviceaccount.com"
      signing_key_id               = "projects/aidash-production-fixture/locations/us-central1/keyRings/aidash-production-capability/cryptoKeys/capability"
    } }
  }
  assert {
    condition = (
      keys(module.credential_broker) == ["production"] &&
      module.credential_broker.production.service_account == "aidash-production-broker@aidash-production-fixture.iam.gserviceaccount.com" &&
      output.secret_prefix.production == "aidash-production-cred-"
    )
    error_message = "Production capabilities and Provider Credential prefixes must bind its dedicated bootstrap broker."
  }
}
run "production_rejects_shared_project" {
  command = plan
  variables {
    nonproduction_project_id = "aidash-production-fixture"
    staging_project_id       = "aidash-production-fixture"
  }
  expect_failures = [var.nonproduction_project_id]
}
run "production_rejects_shared_state" {
  command = plan
  variables { nonproduction_state_bucket = "aidash-production-state" }
  expect_failures = [var.nonproduction_state_bucket]
}
run "production_rejects_stop" {
  command = plan
  variables {
    environments = { production = {
      kind        = "production"
      incarnation = "aaaaaaaaaaaa"
      generation  = 1
      running     = false
      published   = false
      spot        = false
    } }
  }
  expect_failures = [var.environments]
}
run "production_rejects_more_nodes_than_the_autoscaler_survives_losing" {
  command = plan
  variables {
    environments = { production = {
      kind        = "production"
      incarnation = "aaaaaaaaaaaa"
      generation  = 1
      running     = true
      published   = false
      spot        = false
      nodes       = 3
    } }
  }
  expect_failures = [var.environments]
}
run "production_rejects_spot" {
  command = plan
  variables {
    environments = { production = {
      kind        = "production"
      incarnation = "aaaaaaaaaaaa"
      generation  = 1
      running     = true
      published   = false
      spot        = true
    } }
  }
  expect_failures = [var.environments]
}
run "production_rejects_nonproduction_identity" {
  command = plan
  variables {
    environments = { test = {
      kind        = "test"
      incarnation = "aaaaaaaaaaaa"
      generation  = 1
      running     = true
      published   = false
      spot        = false
    } }
  }
  expect_failures = [var.environments]
}

run "production_rejects_unrelated_staging_project" {
  command = plan
  variables { staging_project_id = "aidash-unrelated-fixture" }
  expect_failures = [var.staging_project_id]
}

run "production_rejects_legacy_host_flag" {
  command = plan
  variables {
    environments = { production = {
      kind            = "production"
      legacy_hostname = true
      incarnation     = "aaaaaaaaaaaa"
      generation      = 1
      running         = true
      published       = false
      spot            = false
    } }
  }
  expect_failures = [var.environments]
}
