terraform {
  required_version = ">= 1.9, < 2.0"
  required_providers {
    google      = { source = "hashicorp/google", version = "~> 7.0" }
    google-beta = { source = "hashicorp/google-beta", version = "~> 7.0" }
  }
}
provider "google" { project = var.project_id }
provider "google-beta" { project = var.project_id }

variable "project_id" {
  type = string
  validation {
    condition     = can(regex("^aidash-drill-[a-f0-9]{12}$", var.project_id))
    error_message = "Each drill creates its own uniquely named project."
  }
}
variable "billing_account" { type = string }
variable "folder_id" { type = string }
variable "region" { type = string }
variable "backup_plan" { type = string }
variable "operator_cidr" {
  type = string
  validation {
    condition     = can(cidrhost(var.operator_cidr, 0)) && endswith(var.operator_cidr, "/32")
    error_message = "Supply the operator's single IPv4 address as a /32."
  }
}
variable "namespace" { type = string }

locals {
  source_project  = split("/", var.backup_plan)[1]
  backup_location = split("/", var.backup_plan)[3]
}
resource "google_project" "drill" {
  project_id      = var.project_id
  name            = var.project_id
  billing_account = var.billing_account
  folder_id       = var.folder_id
  deletion_policy = "DELETE"
  labels          = { application = "aidash", lifecycle = "restore-drill" }
  lifecycle {
    precondition {
      condition     = local.source_project != var.project_id
      error_message = "The restore target must be separate from the backup project."
    }
  }
}
resource "google_project_service" "drill" {
  for_each           = toset(["compute.googleapis.com", "container.googleapis.com", "gkebackup.googleapis.com", "artifactregistry.googleapis.com"])
  project            = google_project.drill.project_id
  service            = each.value
  disable_on_destroy = false
}
resource "google_project_service_identity" "backup" {
  provider   = google-beta
  project    = google_project.drill.project_id
  service    = "gkebackup.googleapis.com"
  depends_on = [google_project_service.drill]
}
resource "google_project_iam_member" "backup" {
  project = google_project.drill.project_id
  role    = "roles/gkebackup.serviceAgent"
  member  = google_project_service_identity.backup.member
}
resource "google_project_iam_member" "source" {
  project    = local.source_project
  role       = "roles/gkebackup.crossProjectServiceAgent"
  member     = "serviceAccount:service-${google_project.drill.number}@container-engine-robot.iam.gserviceaccount.com"
  depends_on = [google_project_service.drill]
}
resource "google_compute_network" "drill" {
  project                 = google_project.drill.project_id
  name                    = "restore-drill"
  auto_create_subnetworks = false
  depends_on              = [google_project_service.drill]
}
resource "google_compute_subnetwork" "drill" {
  project                  = google_project.drill.project_id
  name                     = "restore-drill"
  region                   = var.region
  network                  = google_compute_network.drill.id
  ip_cidr_range            = "10.240.0.0/20"
  private_ip_google_access = true
  secondary_ip_range {
    range_name    = "pods"
    ip_cidr_range = "10.244.0.0/16"
  }
  secondary_ip_range {
    range_name    = "services"
    ip_cidr_range = "10.245.0.0/20"
  }
}
// No peering, VPN, NAT or production network is attached. Restores below also
// exclude every workload; a person installs the pinned sealed release later.
resource "google_service_account" "nodes" {
  project      = google_project.drill.project_id
  account_id   = "drill-nodes"
  display_name = "Isolated restore drill nodes"
}
resource "google_project_iam_member" "nodes" {
  for_each = toset(["roles/logging.logWriter", "roles/monitoring.metricWriter", "roles/artifactregistry.reader"])
  project  = google_project.drill.project_id
  role     = each.value
  member   = google_service_account.nodes.member
}
resource "google_artifact_registry_repository" "drill" {
  project       = google_project.drill.project_id
  location      = var.region
  repository_id = "restore-drill"
  format        = "DOCKER"
  depends_on    = [google_project_service.drill]
}
resource "google_container_cluster" "drill" {
  project             = google_project.drill.project_id
  name                = "restore-drill"
  location            = var.region
  initial_node_count  = 1
  deletion_protection = false
  network             = google_compute_network.drill.id
  subnetwork          = google_compute_subnetwork.drill.id
  node_config {
    machine_type    = "e2-standard-4"
    image_type      = "UBUNTU_CONTAINERD"
    disk_size_gb    = 50
    service_account = google_service_account.nodes.email
    oauth_scopes    = ["https://www.googleapis.com/auth/cloud-platform"]
  }
  ip_allocation_policy {
    cluster_secondary_range_name  = "pods"
    services_secondary_range_name = "services"
  }
  private_cluster_config {
    enable_private_nodes    = true
    enable_private_endpoint = false
    master_ipv4_cidr_block  = "172.16.0.0/28"
  }
  master_authorized_networks_config {
    cidr_blocks {
      cidr_block   = var.operator_cidr
      display_name = "drill operator"
    }
  }
  addons_config {
    gke_backup_agent_config { enabled = true }
  }
  depends_on = [google_project_iam_member.backup, google_project_iam_member.nodes]
}
resource "google_gke_backup_restore_channel" "drill" {
  project             = local.source_project
  location            = local.backup_location
  name                = var.project_id
  destination_project = "projects/${google_project.drill.number}"
  deletion_policy     = "DELETE"
  depends_on          = [google_project_iam_member.source]
}
resource "google_gke_backup_restore_plan" "drill" {
  project         = google_project.drill.project_id
  location        = var.region
  name            = "pre-update-drill"
  backup_plan     = var.backup_plan
  cluster         = google_container_cluster.drill.id
  deletion_policy = "DELETE"
  restore_config {
    // Per-Restore exclusion filters require a MERGE mode. The destination is
    // a new project; this never replaces existing production resources.
    namespaced_resource_restore_mode = "MERGE_SKIP_ON_CONFLICT"
    volume_data_restore_policy       = "RESTORE_VOLUME_DATA_FROM_BACKUP"
    selected_namespaces { namespaces = [var.namespace] }
    cluster_resource_restore_scope { no_group_kinds = true }
  }
  depends_on = [google_gke_backup_restore_channel.drill]
}
output "drill" {
  value = {
    project_id   = google_project.drill.project_id
    cluster      = google_container_cluster.drill.id
    restore_plan = google_gke_backup_restore_plan.drill.id
  }
}
