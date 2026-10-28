// Nonproduction shares one zonal cluster. Production uses a separate regional
// control plane and keeps its retained disks on regional disks in two zones:
// its pools run in the first zone, and standby pools in the second scale up
// from zero only when the first is lost (ADR 0067).
locals {
  zone             = "us-central1-a"
  standby_zones    = var.deployment_tier == "production" ? ["us-central1-b"] : []
  pod_cidr         = "10.44.0.0/14"
  any_running      = var.deployment_tier == "production" || anytrue([for e in values(var.environments) : e.running])
  cluster_location = var.deployment_tier == "production" ? "us-central1" : local.zone
  cloud_scope      = ["https://www.googleapis.com/auth/cloud-platform"]
  system_labels    = { "aidash.run/pool" = "system" }
}

resource "google_compute_network" "aidash" {
  name                    = "aidash"
  auto_create_subnetworks = false
}
resource "google_compute_subnetwork" "aidash" {
  name                     = "aidash"
  region                   = "us-central1"
  network                  = google_compute_network.aidash.id
  ip_cidr_range            = "10.40.0.0/22"
  private_ip_google_access = true
  secondary_ip_range {
    range_name    = "pods"
    ip_cidr_range = local.pod_cidr
  }
  secondary_ip_range {
    range_name    = "services"
    ip_cidr_range = "10.48.0.0/20"
  }
}
// Private nodes reach the internet only through NAT.
resource "google_compute_router" "aidash" {
  name    = "aidash"
  region  = "us-central1"
  network = google_compute_network.aidash.id
}
resource "google_compute_router_nat" "aidash" {
  name                               = "aidash"
  router                             = google_compute_router.aidash.name
  region                             = google_compute_router.aidash.region
  nat_ip_allocate_option             = "AUTO_ONLY"
  source_subnetwork_ip_ranges_to_nat = "ALL_SUBNETWORKS_ALL_IP_RANGES"
}

// Host-network Pods can obtain the node identity's token, so it holds no Aidash
// permission: only GKE's node telemetry role and release image pulls.
resource "google_service_account" "nodes" {
  count        = var.deployment_tier == "nonproduction" ? 1 : 0
  account_id   = "aidash-gke-nodes"
  display_name = "Aidash GKE nodes"
}
// Preserve existing nonproduction account state when introducing tier ownership.
moved {
  from = google_service_account.nodes
  to   = google_service_account.nodes[0]
}
data "google_service_account" "production_nodes" {
  count      = var.deployment_tier == "production" ? 1 : 0
  project    = var.project_id
  account_id = "aidash-gke-nodes"
}
locals {
  node_service_account_email = var.deployment_tier == "production" ? data.google_service_account.production_nodes[0].email : google_service_account.nodes[0].email
  node_service_account_name  = var.deployment_tier == "production" ? data.google_service_account.production_nodes[0].name : google_service_account.nodes[0].name
}
resource "google_project_iam_member" "nodes" {
  project = var.project_id
  role    = "roles/container.defaultNodeServiceAccount"
  member  = "serviceAccount:${local.node_service_account_email}"
}
resource "google_artifact_registry_repository_iam_member" "nodes" {
  project    = var.project_id
  location   = "us-central1"
  repository = "aidash"
  role       = "roles/artifactregistry.reader"
  member     = "serviceAccount:${local.node_service_account_email}"
}
resource "google_service_account_iam_member" "nodes_deploy" {
  service_account_id = local.node_service_account_name
  role               = "roles/iam.serviceAccountUser"
  member             = "serviceAccount:${var.deploy_service_account}"
}

resource "google_container_cluster" "aidash" {
  name     = "aidash"
  location = local.cluster_location
  // Zonal clusters include their primary zone implicitly; only regional
  // production needs an explicit initial node location.
  node_locations           = var.deployment_tier == "production" ? [local.zone] : null
  network                  = google_compute_network.aidash.id
  subnetwork               = google_compute_subnetwork.aidash.id
  min_master_version       = "1.34"
  datapath_provider        = "ADVANCED_DATAPATH"
  deletion_protection      = true
  remove_default_node_pool = true
  initial_node_count       = 1
  release_channel { channel = "REGULAR" }
  workload_identity_config { workload_pool = "${var.project_id}.svc.id.goog" }
  ip_allocation_policy {
    cluster_secondary_range_name  = "pods"
    services_secondary_range_name = "services"
  }
  private_cluster_config {
    enable_private_nodes    = true
    enable_private_endpoint = false
  }
  // Automation reaches the control plane only through the IAM-authorized DNS
  // endpoint; the IP endpoint admits no authorized network.
  control_plane_endpoints_config {
    dns_endpoint_config { allow_external_traffic = true }
    ip_endpoints_config { enabled = true }
  }
  master_authorized_networks_config { gcp_public_cidrs_access_enabled = false }
  addons_config {
    gce_persistent_disk_csi_driver_config { enabled = true }
    gke_backup_agent_config { enabled = var.gke_backup_enabled }
    // Fails production's StatefulSets over to the standby zone, force-attaching
    // their regional disks there (ADR 0067).
    stateful_ha_config { enabled = var.deployment_tier == "production" }
  }
  // The transient default pool is deleted after creation; keep it off the
  // Compute default service account.
  node_config {
    service_account = local.node_service_account_email
    oauth_scopes    = local.cloud_scope
    workload_metadata_config { mode = "GKE_METADATA" }
  }
  lifecycle {
    prevent_destroy = true
    ignore_changes  = [node_config]
  }
  depends_on = [
    google_service_account_iam_member.nodes_deploy, google_project_iam_member.nodes,
    google_artifact_registry_repository_iam_member.nodes, google_compute_router_nat.aidash,
  ]
}

// Untainted cluster add-ons run here only while some Environment runs.
resource "google_container_node_pool" "system" {
  name           = "aidash-system"
  location       = google_container_cluster.aidash.location
  cluster        = google_container_cluster.aidash.name
  node_locations = [local.zone]
  node_count     = local.any_running ? 1 : 0
  management {
    auto_repair  = true
    auto_upgrade = true
  }
  node_config {
    # GKE's own add-ons already request about 940m CPU, all an e2-medium can
    # allocate, so konnectivity-agent stayed Pending and `kubectl exec`/logs
    # failed, crash-looping the Runner's isolation probe. e2-standard-2 leaves
    # room for every add-on; it uses two E2 vCPUs of the quota.
    machine_type = "e2-standard-2"
    # GKE's 100 GB pd-balanced default counts against the regional SSD quota
    # (250 GB by default) shared with every Environment pool and retained disk.
    disk_type       = "pd-standard"
    disk_size_gb    = 30
    service_account = local.node_service_account_email
    oauth_scopes    = local.cloud_scope
    labels          = local.system_labels
    resource_labels = { application = "aidash", lifecycle = var.deployment_tier }
    workload_metadata_config { mode = "GKE_METADATA" }
  }
}

// Production's system pool in the standby zone, at zero nodes until the first
// zone is lost and kube-dns and the other add-ons have nowhere else to run.
resource "google_container_node_pool" "system_standby" {
  count          = length(local.standby_zones)
  name           = "aidash-system-standby"
  location       = google_container_cluster.aidash.location
  cluster        = google_container_cluster.aidash.name
  node_locations = [local.standby_zones[count.index]]
  autoscaling {
    total_min_node_count = 0
    total_max_node_count = 1
  }
  management {
    auto_repair  = true
    auto_upgrade = true
  }
  node_config {
    # Sized like the primary system pool; see its machine type.
    machine_type = "e2-standard-2"
    # GKE's 100 GB pd-balanced default counts against the regional SSD quota
    # (250 GB by default) shared with every Environment pool and retained disk.
    disk_type       = "pd-standard"
    disk_size_gb    = 30
    service_account = local.node_service_account_email
    oauth_scopes    = local.cloud_scope
    labels          = local.system_labels
    resource_labels = { application = "aidash", lifecycle = var.deployment_tier }
    workload_metadata_config { mode = "GKE_METADATA" }
  }
}

// ACME account/certificate storage outlives nonproduction incarnations. Production
// has its own copy in a separate project/backend; never share it across tiers.
resource "google_compute_disk" "preview_tls" {
  name   = "aidash-preview-tls"
  zone   = local.zone
  type   = "pd-balanced"
  size   = 10
  labels = { application = "aidash", lifecycle = var.deployment_tier, purpose = "preview-tls" }
  lifecycle { prevent_destroy = true }
}

output "cluster" {
  value = {
    name          = google_container_cluster.aidash.name
    location      = google_container_cluster.aidash.location
    project_id    = var.project_id
    workload_pool = google_container_cluster.aidash.workload_identity_config[0].workload_pool
    pod_cidr      = local.pod_cidr
    // Where regional retained disks replicate: the pools' zone first.
    zones = concat([local.zone], local.standby_zones)
  }
}
output "preview_tls_volume_handle" {
  value = "projects/${var.project_id}/zones/${google_compute_disk.preview_tls.zone}/disks/${google_compute_disk.preview_tls.name}"
}
