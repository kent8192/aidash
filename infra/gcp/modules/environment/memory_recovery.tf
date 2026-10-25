// A second failure domain for the Home's non-body floors and expiring bodies.
// Provisioning is separate from the supervised activation and lost-ledger drill.
locals {
  memory_recovery_required = var.deployment_tier == "production" || var.environment.recovery_protected
  memory_recovery_roles    = local.memory_recovery_required ? toset(["backup", "prune"]) : toset([])
  // The full project/environment hash avoids collisions after bounding the prefix.
  memory_bucket_prefix = "${substr(var.project_id, 0, 20)}-aidash-${substr(sha256("${var.project_id}/${local.name}"), 0, 16)}"
}
resource "google_storage_bucket" "memory_history" {
  count                       = local.memory_recovery_required ? 1 : 0
  project                     = var.project_id
  name                        = "${local.memory_bucket_prefix}-memory-history"
  location                    = "US-CENTRAL1"
  uniform_bucket_level_access = true
  public_access_prevention    = "enforced"
  force_destroy               = false
  labels                      = local.labels
  lifecycle { prevent_destroy = true }
}
resource "google_storage_bucket" "memory_archives" {
  count                       = local.memory_recovery_required ? 1 : 0
  project                     = var.project_id
  name                        = "${local.memory_bucket_prefix}-memory-archives"
  location                    = "US-CENTRAL1"
  uniform_bucket_level_access = true
  public_access_prevention    = "enforced"
  force_destroy               = false
  default_event_based_hold    = false
  labels                      = local.labels
  // A logical deletion must not leave an undeletable retained body copy.
  soft_delete_policy { retention_duration_seconds = 0 }
  versioning { enabled = false }
  lifecycle { prevent_destroy = true }
}
resource "google_service_account" "memory_jobs" {
  for_each     = local.memory_recovery_roles
  project      = var.project_id
  account_id   = "ad${substr(sha256(local.name), 0, 19)}-m${each.key}"
  display_name = "Aidash ${var.environment_id} memory ${each.key}"
}
resource "google_service_account_iam_member" "memory_jobs" {
  for_each           = google_service_account.memory_jobs
  service_account_id = each.value.name
  role               = "roles/iam.workloadIdentityUser"
  member             = "serviceAccount:${var.cluster.workload_pool}[${local.namespace}/app-aidash-memory-${each.key}]"
}
resource "google_storage_bucket_iam_member" "memory_history_writer" {
  for_each = local.memory_recovery_required ? google_service_account.workload : {}
  bucket   = google_storage_bucket.memory_history[0].name
  role     = "projects/${var.project_id}/roles/aidashMemoryHistory"
  member   = "serviceAccount:${each.value.email}"
}
resource "google_storage_bucket_iam_member" "memory_backup_history" {
  count  = local.memory_recovery_required ? 1 : 0
  bucket = google_storage_bucket.memory_history[0].name
  role   = "roles/storage.objectViewer"
  member = "serviceAccount:${google_service_account.memory_jobs["backup"].email}"
}
resource "google_storage_bucket_iam_member" "memory_backup_bodies" {
  count  = local.memory_recovery_required ? 1 : 0
  bucket = google_storage_bucket.memory_archives[0].name
  role   = "projects/${var.project_id}/roles/aidashMemoryHistory"
  member = "serviceAccount:${google_service_account.memory_jobs["backup"].email}"
}
resource "google_storage_bucket_iam_member" "memory_prune_bodies" {
  count  = local.memory_recovery_required ? 1 : 0
  bucket = google_storage_bucket.memory_archives[0].name
  role   = "projects/${var.project_id}/roles/aidashMemoryPrune"
  member = "serviceAccount:${google_service_account.memory_jobs["prune"].email}"
}
output "memory_recovery" {
  value = {
    required               = local.memory_recovery_required
    history_bucket         = try(google_storage_bucket.memory_history[0].name, "")
    archive_bucket         = try(google_storage_bucket.memory_archives[0].name, "")
    backup_service_account = try(google_service_account.memory_jobs["backup"].email, "")
    prune_service_account  = try(google_service_account.memory_jobs["prune"].email, "")
  }
}
