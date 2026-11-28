# SPDX-FileCopyrightText: 2026 kent8192
# SPDX-License-Identifier: BUSL-1.1
# See enterprise/LICENSE.

variable "egress_ip_groups" {
  description = "Operator-managed Egress IP groups and their per-Environment Tenant assignments."
  type = map(object({
    node_cidr = string
    pod_cidr  = string
    # A public address supplies at most 64,512 usable ports to this NAT.
    address_count = optional(number, 1)
    environments = map(object({
      clusterIP = string
      tenants   = optional(set(string), [])
    }))
  }))
  default = {}
  validation {
    condition = length(var.egress_ip_groups) <= 8 && alltrue([
      for name, group in var.egress_ip_groups :
      can(regex("^[a-z][a-z0-9-]{0,38}[a-z0-9]$", name))
      && can(cidrnetmask(group.node_cidr)) && can(cidrnetmask(group.pod_cidr))
      && group.address_count >= 1 && group.address_count == floor(group.address_count)
      && alltrue([for assignment in values(group.environments) : name == "shared" ? length(assignment.tenants) == 0 : length(assignment.tenants) > 0])
    ])
    error_message = "Use up to eight DNS-named Egress IP groups with IPv4 ranges and a positive whole address count. Assign Tenants only to dedicated groups."
  }
}

locals {
  egress_addresses = merge({}, [
    for name, group in var.egress_ip_groups : {
      for i in range(group.address_count) : "${name}-${i}" => name
    }
  ]...)
}

resource "google_compute_subnetwork" "egress" {
  for_each                 = var.egress_ip_groups
  name                     = "aidash-egress-${each.key}"
  region                   = google_compute_subnetwork.aidash.region
  network                  = google_compute_network.aidash.id
  ip_cidr_range            = each.value.node_cidr
  private_ip_google_access = true
  secondary_ip_range {
    range_name    = "egress-${each.key}-pods"
    ip_cidr_range = each.value.pod_cidr
  }
}

resource "google_compute_address" "egress" {
  for_each     = local.egress_addresses
  name         = "aidash-egress-${each.key}"
  region       = google_compute_router.aidash.region
  address_type = "EXTERNAL"
  network_tier = "PREMIUM"
}

resource "google_compute_router_nat" "egress" {
  for_each                            = var.egress_ip_groups
  name                                = "aidash-egress-${each.key}"
  router                              = google_compute_router.aidash.name
  region                              = google_compute_router.aidash.region
  nat_ip_allocate_option              = "MANUAL_ONLY"
  nat_ips                             = [for key, group in local.egress_addresses : google_compute_address.egress[key].self_link if group == each.key]
  source_subnetwork_ip_ranges_to_nat  = "LIST_OF_SUBNETWORKS"
  enable_dynamic_port_allocation      = true
  min_ports_per_vm                    = 1024
  max_ports_per_vm                    = 65536
  enable_endpoint_independent_mapping = false
  # Cover the node IP after masquerading and Pod alias IPs when it is disabled.
  subnetwork {
    name                    = google_compute_subnetwork.egress[each.key].id
    source_ip_ranges_to_nat = ["ALL_IP_RANGES"]
  }
  # Stop the former ALL_SUBNETWORKS NAT from claiming the dedicated range
  # before creating a second NAT on the same router.
  depends_on = [google_compute_router_nat.aidash]
}

resource "google_container_node_pool" "egress" {
  for_each           = var.egress_ip_groups
  name               = "aidash-egress-${each.key}"
  cluster            = google_container_cluster.aidash.name
  location           = google_container_cluster.aidash.location
  node_locations     = concat([local.zone], local.standby_zones)
  initial_node_count = 0
  autoscaling {
    total_min_node_count = 0
    total_max_node_count = 10
  }
  management {
    auto_repair  = true
    auto_upgrade = true
  }
  network_config {
    subnetwork           = google_compute_subnetwork.egress[each.key].id
    pod_range            = google_compute_subnetwork.egress[each.key].secondary_ip_range[0].range_name
    enable_private_nodes = true
  }
  node_config {
    machine_type    = "e2-standard-4"
    image_type      = "UBUNTU_CONTAINERD"
    disk_type       = "pd-standard"
    disk_size_gb    = 30
    service_account = local.node_service_account_email
    oauth_scopes    = local.cloud_scope
    labels          = { "aidash.run/egress-ip-group" = each.key }
    taint {
      key    = "aidash.run/egress-ip-group"
      value  = each.key
      effect = "NO_SCHEDULE"
    }
    workload_metadata_config { mode = "GKE_METADATA" }
  }
  depends_on = [google_compute_router_nat.egress]
}

output "egress_ip_groups" {
  description = "Dedicated Egress IP group public addresses for upstream allowlists."
  value = {
    for name, group in var.egress_ip_groups : name => {
      node_pool = google_container_node_pool.egress[name].name
      addresses = [for key, assigned in local.egress_addresses : google_compute_address.egress[key].address if assigned == name]
    }
  }
}
