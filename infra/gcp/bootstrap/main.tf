locals {
  services = toset([
    "artifactregistry.googleapis.com", "compute.googleapis.com", "iam.googleapis.com",
    "iamcredentials.googleapis.com", "iap.googleapis.com", "oslogin.googleapis.com",
    "secretmanager.googleapis.com", "storage.googleapis.com", "sts.googleapis.com",
    "cloudresourcemanager.googleapis.com",
    "run.googleapis.com", "cloudkms.googleapis.com",
  ])
  workflow_ref = "${var.repository}/.github/workflows/gcp-environments.yml@refs/heads/${var.trusted_branch}"
}

resource "google_project_service" "required" {
  for_each           = local.services
  project            = var.project_id
  service            = each.value
  disable_on_destroy = false
}

resource "google_storage_bucket" "state" {
  name                        = var.state_bucket_name
  location                    = "US-CENTRAL1"
  uniform_bucket_level_access = true
  public_access_prevention    = "enforced"
  force_destroy               = false
  versioning { enabled = true }
  lifecycle_rule {
    condition { num_newer_versions = 10 }
    action { type = "Delete" }
  }
  lifecycle { prevent_destroy = true }
  depends_on = [google_project_service.required]
}

resource "google_storage_bucket" "releases" {
  name                        = var.release_bucket_name
  location                    = "US-CENTRAL1"
  uniform_bucket_level_access = true
  public_access_prevention    = "enforced"
  force_destroy               = false
  lifecycle { prevent_destroy = true }
  depends_on = [google_project_service.required]
}

resource "google_artifact_registry_repository" "images" {
  location      = "us-central1"
  repository_id = "aidash"
  format        = "DOCKER"
  description   = "Digest-pinned Aidash nonproduction images; never retire with a PR."
  lifecycle { prevent_destroy = true }
  depends_on = [google_project_service.required]
}

resource "google_service_account" "automation" {
  for_each     = toset(["deploy", "publish"])
  account_id   = "aidash-${each.key}"
  display_name = "Aidash trusted ${each.key} workflow"
  depends_on   = [google_project_service.required]
}

resource "google_iam_workload_identity_pool" "github" {
  workload_identity_pool_id = "aidash-github"
  display_name              = "Aidash GitHub Actions"
  depends_on                = [google_project_service.required]
}

resource "google_iam_workload_identity_pool_provider" "github" {
  workload_identity_pool_id          = google_iam_workload_identity_pool.github.workload_identity_pool_id
  workload_identity_pool_provider_id = "github"
  attribute_mapping = {
    "google.subject"          = "assertion.sub"
    "attribute.repository_id" = "assertion.repository_id"
  }
  # Numeric identities prevent repository/owner name reuse. A PR workflow or
  # manually selected feature-branch workflow cannot obtain this identity.
  attribute_condition = "assertion.repository_id == '${var.repository_id}' && assertion.repository_owner_id == '${var.repository_owner_id}' && assertion.workflow_ref == '${local.workflow_ref}' && assertion.ref == 'refs/heads/${var.trusted_branch}'"
  oidc { issuer_uri = "https://token.actions.githubusercontent.com" }
}

resource "google_service_account_iam_member" "github" {
  for_each           = google_service_account.automation
  service_account_id = each.value.name
  role               = "roles/iam.workloadIdentityUser"
  member             = "principalSet://iam.googleapis.com/${google_iam_workload_identity_pool.github.name}/attribute.repository_id/${var.repository_id}"
}

resource "google_project_iam_member" "deploy" {
  for_each = toset([
    "roles/compute.instanceAdmin.v1", "roles/compute.networkAdmin",
    "roles/compute.securityAdmin", "roles/compute.osAdminLogin",
    "roles/iap.tunnelResourceAccessor", "roles/iam.serviceAccountAdmin",
    "roles/secretmanager.admin", "roles/artifactregistry.reader",
    "roles/serviceusage.serviceUsageConsumer",
    "roles/run.admin", "roles/cloudkms.admin",
  ])
  project = var.project_id
  role    = each.value
  member  = "serviceAccount:${google_service_account.automation["deploy"].email}"
}

resource "google_storage_bucket_iam_member" "state" {
  bucket = google_storage_bucket.state.name
  role   = "roles/storage.objectAdmin"
  member = "serviceAccount:${google_service_account.automation["deploy"].email}"
}

resource "google_storage_bucket_iam_member" "release" {
  bucket = google_storage_bucket.releases.name
  role   = "roles/storage.admin"
  member = "serviceAccount:${google_service_account.automation["deploy"].email}"
}

resource "google_artifact_registry_repository_iam_member" "publisher" {
  location   = google_artifact_registry_repository.images.location
  repository = google_artifact_registry_repository.images.repository_id
  role       = "roles/artifactregistry.writer"
  member     = "serviceAccount:${google_service_account.automation["publish"].email}"
}

resource "google_artifact_registry_repository_iam_member" "deploy_policy" {
  location   = google_artifact_registry_repository.images.location
  repository = google_artifact_registry_repository.images.repository_id
  role       = "roles/artifactregistry.admin"
  member     = "serviceAccount:${google_service_account.automation["deploy"].email}"
}
