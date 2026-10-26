terraform {
  required_providers {
    google = {
      source  = "hashicorp/google"
      version = "~> 7.0"
    }
  }
}

locals {
  name      = "aidash-${var.environment_id}-${var.environment.incarnation}"
  namespace = "aidash-${var.environment_id}"
  labels    = { application = "aidash", environment = var.environment_id, lifecycle = var.deployment_tier }
}

// Only this environment's Pods tolerate the taint; stopping scales the pool to zero.
resource "google_container_node_pool" "environment" {
  name           = local.name
  location       = var.cluster.location
  cluster        = var.cluster.name
  node_locations = var.cluster.node_locations
  node_count     = var.environment.running ? var.environment.nodes : 0
  management {
    auto_repair  = true
    auto_upgrade = true
  }
  node_config {
    machine_type = var.environment.machine_type
    image_type   = "UBUNTU_CONTAINERD"
    # Bounded below GKE's 100 GB default, which exhausts the regional SSD quota
    # with two nodes beside the retained disks; images and emptyDirs fit in 50 GB.
    disk_type       = "pd-balanced"
    disk_size_gb    = 50
    spot            = var.environment.spot
    service_account = var.node_service_account
    oauth_scopes    = ["https://www.googleapis.com/auth/cloud-platform"]
    labels          = { "aidash.run/environment" = var.environment_id }
    resource_labels = local.labels
    taint {
      key    = "aidash.run/environment"
      value  = var.environment_id
      effect = "NO_SCHEDULE"
    }
    workload_metadata_config { mode = "GKE_METADATA" }
  }
}

// The same pool in each standby zone: no node while the first zone serves, and
// as many as the first zone runs once its StatefulSets fail over (ADR 0067).
resource "google_container_node_pool" "standby" {
  for_each       = var.environment.running ? toset(var.cluster.standby_zones) : toset([])
  name           = "${local.name}-${substr(sha256(each.key), 0, 4)}"
  location       = var.cluster.location
  cluster        = var.cluster.name
  node_locations = [each.key]
  autoscaling {
    total_min_node_count = 0
    total_max_node_count = var.environment.nodes
  }
  management {
    auto_repair  = true
    auto_upgrade = true
  }
  node_config {
    machine_type = var.environment.machine_type
    image_type   = "UBUNTU_CONTAINERD"
    # Bounded below GKE's 100 GB default, which exhausts the regional SSD quota
    # with two nodes beside the retained disks; images and emptyDirs fit in 50 GB.
    disk_type       = "pd-balanced"
    disk_size_gb    = 50
    spot            = var.environment.spot
    service_account = var.node_service_account
    oauth_scopes    = ["https://www.googleapis.com/auth/cloud-platform"]
    labels          = { "aidash.run/environment" = var.environment_id }
    resource_labels = local.labels
    taint {
      key    = "aidash.run/environment"
      value  = var.environment_id
      effect = "NO_SCHEDULE"
    }
    workload_metadata_config { mode = "GKE_METADATA" }
  }
}

// Server and worker hold this environment's only Aidash permissions, through
// their Kubernetes service accounts in its application namespace.
resource "google_service_account" "workload" {
  for_each     = toset(["server", "worker"])
  account_id   = "ad${substr(sha256(local.name), 0, 20)}-${each.key}"
  display_name = "Aidash ${var.environment_id} ${each.key}"
}
resource "google_service_account_iam_member" "workload_identity" {
  for_each           = google_service_account.workload
  service_account_id = each.value.name
  role               = "roles/iam.workloadIdentityUser"
  member             = "serviceAccount:${var.cluster.workload_pool}[${local.namespace}/app-aidash-${each.key}]"
}

// The controller reads this secret and materializes the namespace's runtime Secret.
resource "google_secret_manager_secret" "runtime" {
  secret_id = "${local.name}-runtime"
  labels    = local.labels
  replication {
    auto {}
  }
}

locals {
  provider_credentials = var.byok_project_id != "" ? {
    fingerprint_key = { env = "AIDASH_PROVIDER_FINGERPRINT_KEY" }
    store = {
      kind            = "secret_manager"
      byok_project_id = var.byok_project_id
      environment_id  = var.environment_id
    }
    broker               = var.broker
    tenant_secret_broker = var.tenant_secret_broker
  } : null
}
output "namespace" { value = local.namespace }
output "node_pool" { value = google_container_node_pool.environment.name }
output "standby_node_pools" {
  value = { for zone, pool in google_container_node_pool.standby : zone => {
    name     = pool.name
    min      = pool.autoscaling[0].total_min_node_count
    max      = pool.autoscaling[0].total_max_node_count
    taint    = pool.node_config[0].taint[0].value
    location = one(pool.node_locations)
  } }
}
output "hostname" { value = var.hostname }
output "provider_credentials" { value = local.provider_credentials }
output "runtime_secret" { value = google_secret_manager_secret.runtime.secret_id }
output "server_service_account" { value = google_service_account.workload["server"].email }
output "worker_service_account" { value = google_service_account.workload["worker"].email }
