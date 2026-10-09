terraform {
  required_providers {
    google = {
      source  = "hashicorp/google"
      version = "~> 7.0"
    }
  }
}

locals {
  name   = "aidash-${var.environment_id}-${var.environment.incarnation}"
  zone   = "us-central1-a"
  labels = { application = "aidash", environment = var.environment_id, lifecycle = "nonproduction" }
}

resource "google_compute_network" "environment" {
  name                    = local.name
  auto_create_subnetworks = false
}
resource "google_compute_subnetwork" "environment" {
  name                     = local.name
  region                   = "us-central1"
  network                  = google_compute_network.environment.id
  ip_cidr_range            = "10.40.0.0/24"
  private_ip_google_access = true
}
resource "google_compute_firewall" "web" {
  name          = "${local.name}-web"
  network       = google_compute_network.environment.name
  source_ranges = ["0.0.0.0/0"]
  target_tags   = [local.name]
  allow {
    protocol = "tcp"
    ports    = ["80", "443"]
  }
}
resource "google_compute_firewall" "iap" {
  name          = "${local.name}-iap"
  network       = google_compute_network.environment.name
  source_ranges = ["35.235.240.0/20"]
  target_tags   = [local.name]
  allow {
    protocol = "tcp"
    ports    = ["22"]
  }
}

resource "google_service_account" "runtime" {
  account_id   = "ad-${substr(sha256(local.name), 0, 24)}"
  display_name = "Aidash ${var.environment_id} runtime"
}
resource "google_service_account_iam_member" "deploy" {
  service_account_id = google_service_account.runtime.name
  role               = "roles/iam.serviceAccountUser"
  member             = "serviceAccount:${var.deploy_service_account}"
}
resource "google_artifact_registry_repository_iam_member" "pull" {
  project    = var.project_id
  location   = "us-central1"
  repository = "aidash"
  role       = "roles/artifactregistry.reader"
  member     = "serviceAccount:${google_service_account.runtime.email}"
}
resource "google_storage_bucket_iam_member" "bundle" {
  bucket = var.release_bucket
  role   = "roles/storage.objectViewer"
  member = "serviceAccount:${google_service_account.runtime.email}"
  condition {
    title      = "only-immutable-bundles"
    expression = "resource.name.startsWith('projects/_/buckets/${var.release_bucket}/objects/bundles/')"
  }
}
resource "google_secret_manager_secret" "runtime" {
  secret_id = "${local.name}-runtime"
  labels    = local.labels
  replication {
    auto {}
  }
}
resource "google_secret_manager_secret_iam_member" "runtime" {
  secret_id = google_secret_manager_secret.runtime.id
  role      = "roles/secretmanager.secretAccessor"
  member    = "serviceAccount:${google_service_account.runtime.email}"
}

resource "google_compute_disk" "boot" {
  name   = "${local.name}-boot"
  zone   = local.zone
  type   = "pd-balanced"
  size   = var.environment.boot_disk_gib
  image  = "projects/ubuntu-os-cloud/global/images/family/ubuntu-2404-lts-amd64"
  labels = local.labels
  # An updated image family must not silently replace a retained boot disk.
  lifecycle { ignore_changes = [image] }
}
resource "google_compute_disk" "data" {
  name   = "${local.name}-data"
  zone   = local.zone
  type   = "pd-balanced"
  size   = var.environment.data_disk_gib
  labels = local.labels
}
resource "google_compute_instance" "host" {
  count                     = var.environment.vm_present ? 1 : 0
  name                      = local.name
  zone                      = local.zone
  machine_type              = var.environment.machine_type
  desired_status            = var.environment.running ? "RUNNING" : "TERMINATED"
  allow_stopping_for_update = false
  labels                    = local.labels
  tags                      = [local.name]
  boot_disk {
    source      = google_compute_disk.boot.id
    auto_delete = false
  }
  attached_disk {
    source      = google_compute_disk.data.id
    device_name = "aidash-data"
  }
  dynamic "attached_disk" {
    for_each = var.environment.kind == "pr" && var.environment.running ? [var.preview_tls_disk] : []
    content {
      source      = attached_disk.value
      device_name = "aidash-preview-tls"
    }
  }
  network_interface {
    subnetwork = google_compute_subnetwork.environment.id
    access_config {}
  }
  service_account {
    email  = google_service_account.runtime.email
    scopes = ["cloud-platform"]
  }
  scheduling {
    provisioning_model          = var.environment.spot ? "SPOT" : "STANDARD"
    automatic_restart           = false
    on_host_maintenance         = var.environment.spot ? "TERMINATE" : "MIGRATE"
    instance_termination_action = var.environment.spot ? "STOP" : null
  }
  shielded_instance_config {
    enable_secure_boot          = true
    enable_vtpm                 = true
    enable_integrity_monitoring = true
  }
  metadata = merge({
    enable-oslogin         = "TRUE"
    block-project-ssh-keys = "TRUE"
    serial-port-enable     = "FALSE"
    startup-script = templatefile("${path.module}/startup.sh.tftpl", {
      project  = var.project_id
      bucket   = var.release_bucket
      object   = var.environment.bundle_object
      sha256   = var.environment.bundle_sha256
      hostname = var.hostname
      secret   = google_secret_manager_secret.runtime.secret_id
      preview  = var.environment.kind == "pr"
    })
    }, local.provider_credentials != null ? {
    aidash-provider-credentials = jsonencode(local.provider_credentials)
  } : {})
  depends_on = [
    google_service_account_iam_member.deploy, google_storage_bucket_iam_member.bundle,
    google_artifact_registry_repository_iam_member.pull, google_secret_manager_secret_iam_member.runtime,
  ]
  # Refreshing another environment must never undo Spot preemption or an OS
  # shutdown. The trusted controller performs explicit power operations.
  lifecycle {
    ignore_changes = [desired_status]
    precondition {
      condition     = var.environment.kind != "pr" || var.preview_tls_disk != null
      error_message = "PR environments must use the shared preview TLS disk."
    }
  }
}

output "instance" { value = local.name }
output "zone" { value = local.zone }
output "hostname" { value = var.hostname }
locals {
  provider_credentials = var.byok_project_id != "" ? {
    fingerprint_key = { env = "AIDASH_PROVIDER_FINGERPRINT_KEY" }
    store = {
      kind            = "secret_manager"
      byok_project_id = var.byok_project_id
      environment_id  = var.environment_id
    }
    broker = var.broker
  } : null
}
output "provider_credentials" { value = local.provider_credentials }
output "external_ip" { value = try(google_compute_instance.host[0].network_interface[0].access_config[0].nat_ip, "") }
output "runtime_secret" { value = google_secret_manager_secret.runtime.secret_id }
output "runtime_service_account" { value = google_service_account.runtime.email }
