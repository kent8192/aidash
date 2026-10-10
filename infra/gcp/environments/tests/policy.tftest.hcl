mock_provider "google" {
  mock_resource "google_cloud_run_v2_service" { defaults = { uri = "https://broker.run.app" } }
  mock_resource "google_service_account" {
    defaults = {
      name  = "projects/aidash-fixture/serviceAccounts/fixture@aidash-fixture.iam.gserviceaccount.com"
      email = "fixture@aidash-fixture.iam.gserviceaccount.com"
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
  deploy_service_account = "deploy@aidash-fixture.iam.gserviceaccount.com"
  cloudflare_zone_id     = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
}

run "empty_runs_no_node_or_dns" {
  command = plan
  assert {
    condition     = length(module.environment) == 0 && length(cloudflare_dns_record.environment) == 0 && google_container_node_pool.system.node_count == 0
    error_message = "With no running environment, no VM may run and no DNS may exist."
  }
  assert {
    condition     = length(module.credential_broker) == 0
    error_message = "Brokers require explicit Provider Credential enablement."
  }
  assert {
    condition     = google_compute_disk.preview_tls.name == "aidash-preview-tls" && google_compute_disk.preview_tls.size == 10 && output.preview_tls_volume_handle == "projects/aidash-fixture/zones/us-central1-a/disks/aidash-preview-tls"
    error_message = "The preview TLS store must survive every PR and be addressable as a static volume."
  }
  assert {
    condition = (
      google_container_cluster.aidash.name == "aidash" && google_container_cluster.aidash.location == "us-central1-a" &&
      google_container_cluster.aidash.deletion_protection && google_container_cluster.aidash.remove_default_node_pool &&
      google_container_cluster.aidash.min_master_version == "1.34" &&
      google_container_cluster.aidash.release_channel[0].channel == "REGULAR" &&
      google_container_cluster.aidash.datapath_provider == "ADVANCED_DATAPATH" &&
      google_container_cluster.aidash.workload_identity_config[0].workload_pool == "aidash-fixture.svc.id.goog" &&
      output.cluster == { name = "aidash", location = "us-central1-a", project_id = "aidash-fixture", workload_pool = "aidash-fixture.svc.id.goog", pod_cidr = "10.44.0.0/14" }
    )
    error_message = "One protected zonal Dataplane V2 cluster with Workload Identity hosts every environment."
  }
  assert {
    condition = (
      google_container_cluster.aidash.private_cluster_config[0].enable_private_nodes &&
      google_container_cluster.aidash.control_plane_endpoints_config[0].dns_endpoint_config[0].allow_external_traffic &&
      length(google_container_cluster.aidash.master_authorized_networks_config) == 1 &&
      length(google_container_cluster.aidash.master_authorized_networks_config[0].cidr_blocks) == 0 &&
      !google_container_cluster.aidash.master_authorized_networks_config[0].gcp_public_cidrs_access_enabled &&
      google_compute_subnetwork.aidash.private_ip_google_access &&
      google_compute_router_nat.aidash.source_subnetwork_ip_ranges_to_nat == "ALL_SUBNETWORKS_ALL_IP_RANGES"
    )
    error_message = "Nodes are private behind NAT; the control plane is reachable only through the IAM-checked DNS endpoint."
  }
}

run "identities_are_separated" {
  command = apply
  override_resource {
    target = google_service_account.nodes
    values = {
      name  = "projects/aidash-fixture/serviceAccounts/aidash-gke-nodes@aidash-fixture.iam.gserviceaccount.com"
      email = "aidash-gke-nodes@aidash-fixture.iam.gserviceaccount.com"
    }
  }
  override_resource {
    target = module.environment["test"].google_service_account.workload["server"]
    values = {
      name  = "projects/aidash-fixture/serviceAccounts/server@aidash-fixture.iam.gserviceaccount.com"
      email = "server@aidash-fixture.iam.gserviceaccount.com"
    }
  }
  override_resource {
    target = module.environment["test"].google_service_account.workload["worker"]
    values = {
      name  = "projects/aidash-fixture/serviceAccounts/worker@aidash-fixture.iam.gserviceaccount.com"
      email = "worker@aidash-fixture.iam.gserviceaccount.com"
    }
  }
  variables {
    environments = {
      test = {
        kind        = "test"
        incarnation = "aaaaaaaaaaaa"
        generation  = 1
        running     = true
        published   = false
        spot        = true
        release_sha = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
      }
    }
    credential_brokers = {
      test = {
        enabled                      = true
        byok_project_id              = "aidash-byok-fixture"
        secret_prefix                = "aidash-test-cred-"
        broker_service_account_email = "aidash-test-broker@aidash-fixture.iam.gserviceaccount.com"
        signing_key_id               = "projects/aidash-fixture/locations/us-central1/keyRings/aidash-test-capability/cryptoKeys/capability"
        image                        = "broker@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
      }
    }
  }
  assert {
    condition     = module.credential_broker["test"].service_account == var.credential_brokers["test"].broker_service_account_email
    error_message = "Environment composition must consume the broker SA from bootstrap."
  }
  assert {
    condition = (
      google_kms_crypto_key_iam_member.broker_signer["test"].member == "serviceAccount:worker@aidash-fixture.iam.gserviceaccount.com" &&
      google_kms_crypto_key_iam_member.broker_signer["test"].role == "roles/cloudkms.signer" &&
      google_kms_crypto_key_iam_member.broker_signer["test"].crypto_key_id == module.credential_broker["test"].signing_key
    )
    error_message = "Only the matching worker identity may sign capabilities for its broker."
  }
  assert {
    condition = (
      output.environments["test"].server_service_account == "server@aidash-fixture.iam.gserviceaccount.com" &&
      output.environments["test"].worker_service_account == "worker@aidash-fixture.iam.gserviceaccount.com" &&
      output.environments["test"].gcip.runtime_service_account == "server@aidash-fixture.iam.gserviceaccount.com" &&
      output.environments["test"].namespace == "aidash-test" && output.environments["test"].node_pool == "aidash-test-aaaaaaaaaaaa" &&
      output.environments["test"].hostname == "test.aidash.run" && output.environments["test"].runtime_secret == "aidash-test-aaaaaaaaaaaa-runtime"
    )
    error_message = "Each environment exposes its namespace, pool and distinct server/worker identities."
  }
  assert {
    condition = (
      google_project_iam_member.nodes.role == "roles/container.defaultNodeServiceAccount" &&
      google_project_iam_member.nodes.member == "serviceAccount:aidash-gke-nodes@aidash-fixture.iam.gserviceaccount.com" &&
      google_project_iam_member.nodes.project == var.project_id && length(google_project_iam_member.nodes.condition) == 0 &&
      google_artifact_registry_repository_iam_member.nodes.role == "roles/artifactregistry.reader" && google_artifact_registry_repository_iam_member.nodes.repository == "aidash" &&
      google_artifact_registry_repository_iam_member.nodes.member == "serviceAccount:aidash-gke-nodes@aidash-fixture.iam.gserviceaccount.com" &&
      google_kms_crypto_key_iam_member.broker_signer["test"].member != "serviceAccount:aidash-gke-nodes@aidash-fixture.iam.gserviceaccount.com"
    )
    error_message = "The node identity holds only GKE's node role and release image pulls."
  }
  assert {
    condition = (
      google_service_account_iam_member.nodes_deploy.service_account_id == google_service_account.nodes.name &&
      google_service_account_iam_member.nodes_deploy.role == "roles/iam.serviceAccountUser" &&
      google_service_account_iam_member.nodes_deploy.member == "serviceAccount:${var.deploy_service_account}" &&
      google_container_cluster.aidash.node_config[0].service_account == google_service_account.nodes.email &&
      google_container_node_pool.system.node_config[0].service_account == google_service_account.nodes.email
    )
    error_message = "Deploy must act as the node identity to create node pools; no pool uses the Compute default identity."
  }
  assert {
    condition = (
      google_container_node_pool.system.node_count == 1 &&
      google_container_node_pool.system.node_config[0].machine_type == "e2-medium" &&
      google_container_node_pool.system.node_config[0].labels == tomap({ "aidash.run/pool" = "system" }) &&
      length(google_container_node_pool.system.node_config[0].taint) == 0
    )
    error_message = "A running environment needs the untainted E2 system node."
  }
  assert {
    condition     = output.environments["test"].provider_credentials.broker == module.credential_broker["test"].worker_configuration && output.environments["test"].provider_credentials.store.byok_project_id == var.byok_project_id && output.environments["test"].provider_credentials.store.environment_id == "test" && output.environments["test"].provider_credentials.broker.endpoint == "https://broker.run.app/api/v1"
    error_message = "Environment output must carry the actual Store and broker worker settings."
  }
}

run "dns_requires_published_running_address" {
  command = plan
  variables {
    environments = {
      test = {
        kind        = "test"
        incarnation = "aaaaaaaaaaaa"
        generation  = 1
        running     = true
        published   = true
        spot        = true
        address     = "203.0.113.10"
      }
      pr-7 = {
        kind        = "pr"
        incarnation = "bbbbbbbbbbbb"
        generation  = 1
        running     = true
        published   = true
        spot        = true
      }
      develop = {
        kind        = "develop"
        incarnation = "cccccccccccc"
        generation  = 1
        running     = false
        published   = false
        spot        = false
        address     = "203.0.113.11"
      }
    }
  }
  assert {
    condition = (
      keys(cloudflare_dns_record.environment) == ["test"] &&
      cloudflare_dns_record.environment["test"].content == "203.0.113.10" &&
      cloudflare_dns_record.environment["test"].name == "test.aidash.run" &&
      cloudflare_dns_record.environment["test"].type == "A" && !cloudflare_dns_record.environment["test"].proxied
    )
    error_message = "DNS points at the LoadBalancer address only while published and running."
  }
  assert {
    condition     = google_container_node_pool.system.node_count == 1
    error_message = "Any running environment keeps one system node."
  }
}

run "stopped_retains_environment_without_dns_or_nodes" {
  command = plan
  variables {
    environments = {
      test = {
        kind        = "test"
        incarnation = "aaaaaaaaaaaa"
        generation  = 1
        running     = false
        published   = false
        spot        = true
        address     = "203.0.113.10"
      }
    }
  }
  assert {
    condition     = length(module.environment) == 1 && length(cloudflare_dns_record.environment) == 0 && google_container_node_pool.system.node_count == 0
    error_message = "Stopping keeps the environment's resources while withdrawing DNS and every node."
  }
  assert {
    condition     = output.managed_configuration["test"].nodes == 1 && output.managed_configuration["test"].machine_type == "n2-standard-4" && output.managed_configuration["test"].release_sha == null
    error_message = "Omitted sizing uses one N2 node by default."
  }
}

run "no_broker_in_preview" {
  command = plan
  variables {
    environments = {
      pr-137 = {
        kind        = "pr"
        incarnation = "aaaaaaaaaaaa"
        generation  = 1
        running     = false
        published   = false
        spot        = true
      }
    }
    credential_brokers = {
      pr-137 = {
        enabled                      = true
        byok_project_id              = "aidash-byok-fixture"
        secret_prefix                = "aidash-pr-137-cred-"
        broker_service_account_email = "aidash-pr-137-broker@aidash-fixture.iam.gserviceaccount.com"
        signing_key_id               = "projects/aidash-fixture/locations/us-central1/keyRings/aidash-pr-137-capability/cryptoKeys/capability"
        image                        = "broker@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
      }
    }
  }
  expect_failures = [var.credential_brokers]
}

run "broker_must_use_the_environment_store_project" {
  command = plan
  variables {
    environments = { test = {
      kind        = "test"
      incarnation = "aaaaaaaaaaaa"
      generation  = 1
      running     = false
      published   = false
      spot        = true
    } }
    credential_brokers = {
      test = {
        enabled                      = true
        byok_project_id              = "aidash-byok-other"
        secret_prefix                = "aidash-test-cred-"
        broker_service_account_email = "aidash-test-broker@aidash-fixture.iam.gserviceaccount.com"
        signing_key_id               = "projects/aidash-fixture/locations/us-central1/keyRings/aidash-test-capability/cryptoKeys/capability"
        image                        = "broker@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
      }
    }
  }
  expect_failures = [var.credential_brokers]
}

run "no_production" {
  command = plan
  variables {
    environments = {
      production = {
        kind        = "production"
        incarnation = "aaaaaaaaaaaa"
        generation  = 1
        running     = true
        published   = false
        spot        = false
      }
    }
  }
  expect_failures = [var.environments]
}

run "no_spot_develop" {
  command = plan
  variables {
    environments = {
      develop = {
        kind        = "develop"
        incarnation = "aaaaaaaaaaaa"
        generation  = 1
        running     = true
        published   = false
        spot        = true
      }
    }
  }
  expect_failures = [var.environments]
}

run "environment_pools_are_n2" {
  command = plan
  variables {
    environments = {
      test = {
        kind         = "test"
        incarnation  = "aaaaaaaaaaaa"
        generation   = 1
        running      = true
        published    = false
        spot         = true
        machine_type = "e2-standard-4"
      }
    }
  }
  expect_failures = [var.environments]
}

run "running_pool_needs_a_node" {
  command = plan
  variables {
    environments = {
      test = {
        kind        = "test"
        incarnation = "aaaaaaaaaaaa"
        generation  = 1
        running     = true
        published   = false
        spot        = true
        nodes       = 0
      }
    }
  }
  expect_failures = [var.environments]
}

run "dns_address_is_ipv4" {
  command = plan
  variables {
    environments = {
      test = {
        kind        = "test"
        incarnation = "aaaaaaaaaaaa"
        generation  = 1
        running     = true
        published   = true
        spot        = true
        address     = "test.aidash.run"
      }
    }
  }
  expect_failures = [var.environments]
}

run "no_two_running_previews" {
  command = plan
  variables {
    environments = { for id in ["pr-1", "pr-2"] : id => {
      kind        = "pr"
      incarnation = "aaaaaaaaaaaa"
      generation  = 1
      running     = true
      published   = false
      spot        = true
    } }
  }
  expect_failures = [var.environments]
}
