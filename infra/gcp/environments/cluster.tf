// One shared zonal GKE Standard cluster. Environments own node pools inside it;
// the cluster, network and preview TLS disk outlive every Environment.
locals {
  zone          = "us-central1-a"
  pod_cidr      = "10.44.0.0/14"
  any_running   = anytrue([for e in values(var.environments) : e.running])
  cloud_scope   = ["https://www.googleapis.com/auth/cloud-platform"]
  system_labels = { "aidash.run/pool" = "system" }
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
  account_id   = "aidash-gke-nodes"
  display_name = "Aidash GKE nodes"
}
resource "google_project_iam_member" "nodes" {
  project = var.project_id
  role    = "roles/container.defaultNodeServiceAccount"
  member  = "serviceAccount:${google_service_account.nodes.email}"
}
resource "google_artifact_registry_repository_iam_member" "nodes" {
  project    = var.project_id
  location   = "us-central1"
  repository = "aidash"
  role       = "roles/artifactregistry.reader"
  member     = "serviceAccount:${google_service_account.nodes.email}"
}
resource "google_service_account_iam_member" "nodes_deploy" {
  service_account_id = google_service_account.nodes.name
  role               = "roles/iam.serviceAccountUser"
  member             = "serviceAccount:${var.deploy_service_account}"
}

resource "google_container_cluster" "aidash" {
  name                     = "aidash"
  location                 = local.zone
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
  }
  // The transient default pool is deleted after creation; keep it off the
  // Compute default service account.
  node_config {
    service_account = google_service_account.nodes.email
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
  name       = "aidash-system"
  location   = google_container_cluster.aidash.location
  cluster    = google_container_cluster.aidash.name
  node_count = local.any_running ? 1 : 0
  management {
    auto_repair  = true
    auto_upgrade = true
  }
  node_config {
    machine_type    = "e2-medium"
    service_account = google_service_account.nodes.email
    oauth_scopes    = local.cloud_scope
    labels          = local.system_labels
    resource_labels = { application = "aidash", lifecycle = "nonproduction" }
    workload_metadata_config { mode = "GKE_METADATA" }
  }
}

// The hostname belongs to the preview slot, not to an individual PR. Keep its
// certificate/account store even when the last PR environment is destroyed.
resource "google_compute_disk" "preview_tls" {
  name   = "aidash-preview-tls"
  zone   = local.zone
  type   = "pd-balanced"
  size   = 10
  labels = { application = "aidash", lifecycle = "nonproduction", purpose = "preview-tls" }
  lifecycle { prevent_destroy = true }
}

output "cluster" {
  value = {
    name          = google_container_cluster.aidash.name
    location      = google_container_cluster.aidash.location
    project_id    = var.project_id
    workload_pool = google_container_cluster.aidash.workload_identity_config[0].workload_pool
    pod_cidr      = local.pod_cidr
  }
}
output "preview_tls_volume_handle" {
  value = "projects/${var.project_id}/zones/${google_compute_disk.preview_tls.zone}/disks/${google_compute_disk.preview_tls.name}"
}
