# SPDX-FileCopyrightText: 2026 kent8192
# SPDX-License-Identifier: BUSL-1.1
# See enterprise/LICENSE.

mock_provider "google" {
  mock_resource "google_service_account" {
    defaults = {
      name  = "projects/aidash-fixture/serviceAccounts/aidash-gke-nodes@aidash-fixture.iam.gserviceaccount.com"
      email = "aidash-gke-nodes@aidash-fixture.iam.gserviceaccount.com"
    }
  }
}
mock_provider "cloudflare" {}

variables {
  project_id             = "aidash-fixture"
  byok_project_id        = "aidash-byok-fixture"
  cloudflare_zone_id     = "fixture"
  deploy_service_account = "deploy@aidash-fixture.iam.gserviceaccount.com"
  egress_ip_groups = {
    isolated = {
      node_cidr     = "10.52.0.0/24"
      pod_cidr      = "10.53.0.0/20"
      address_count = 2
      environments = {
        test = { clusterIP = "10.48.0.201", tenants = ["acme"] }
      }
    }
  }
}

run "gateway_groups_use_disjoint_nat_subnets" {
  command = apply

  assert {
    condition = (
      google_compute_router_nat.aidash.enable_dynamic_port_allocation &&
      google_compute_router_nat.aidash.min_ports_per_vm == 1024 &&
      google_compute_router_nat.aidash.max_ports_per_vm == 65536 &&
      !google_compute_router_nat.aidash.enable_endpoint_independent_mapping &&
      one(google_compute_router_nat.aidash.subnetwork).name == google_compute_subnetwork.aidash.id
    )
    error_message = "The default NAT must use dynamic port allocation on the default subnet only."
  }
  assert {
    condition = (
      google_compute_router_nat.egress["isolated"].nat_ip_allocate_option == "MANUAL_ONLY" &&
      length(google_compute_router_nat.egress["isolated"].nat_ips) == 2 &&
      one(google_compute_router_nat.egress["isolated"].subnetwork).name == google_compute_subnetwork.egress["isolated"].id &&
      one(google_compute_router_nat.egress["isolated"].subnetwork).source_ip_ranges_to_nat == toset(["ALL_IP_RANGES"]) &&
      one(google_container_node_pool.egress["isolated"].network_config).subnetwork == google_compute_subnetwork.egress["isolated"].id &&
      one(google_container_node_pool.egress["isolated"].network_config).pod_range == "egress-isolated-pods" &&
      one(one(google_container_cluster.aidash.ip_allocation_policy).additional_ip_ranges_config).subnetwork == google_compute_subnetwork.egress["isolated"].id
    )
    error_message = "The group must place nodes and Pods in its subnet and NAT both ranges through its reserved addresses."
  }
}
