# SPDX-FileCopyrightText: 2026 kent8192
# SPDX-License-Identifier: BUSL-1.1
# See enterprise/LICENSE.

locals {
  services = toset([
    "artifactregistry.googleapis.com", "compute.googleapis.com", "container.googleapis.com",
    "iam.googleapis.com", "iamcredentials.googleapis.com",
    "secretmanager.googleapis.com", "storage.googleapis.com", "sts.googleapis.com",
    "cloudresourcemanager.googleapis.com",
    "run.googleapis.com", "cloudkms.googleapis.com",
  ])
  deploy_branch             = var.deployment_tier == "production" ? "main" : var.automation_branch
  deploy_workflow           = var.deployment_tier == "production" ? "gcp-production.yml" : "gcp-environments.yml"
  deploy_environment        = var.deployment_tier == "production" ? "production" : "nonproduction"
  workflow_ref              = "${var.repository}/.github/workflows/${local.deploy_workflow}@refs/heads/${local.deploy_branch}"
  notification_workflow_ref = "${var.repository}/.github/workflows/deployment-notifications.yml@refs/heads/${var.automation_branch}"
  deploy_assertion          = "assertion.workflow_ref == '${local.workflow_ref}' && assertion.ref == 'refs/heads/${local.deploy_branch}' && assertion.environment == '${local.deploy_environment}'"
  notification_assertion    = "assertion.workflow_ref == '${local.notification_workflow_ref}' && assertion.ref == 'refs/heads/${var.automation_branch}' && assertion.environment == 'deployment-notifications'"
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
  description   = "Digest-pinned Aidash ${var.deployment_tier} images; retained across deployments."
  lifecycle { prevent_destroy = true }
  depends_on = [google_project_service.required]
}

resource "google_service_account" "automation" {
  for_each     = toset(["deploy", "publish", "notification-reader"])
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
    "attribute.workflow_role" = "assertion.environment == 'deployment-notifications' ? 'notification-reader' : 'deploy'"
  }
  # Numeric identities prevent repository/owner name reuse. A PR workflow or
  # manually selected feature-branch workflow cannot obtain this identity. The
  # environment claim exists only when the job declares a GitHub Environment, so
  # the protection rules of the tier's Environment (#151) gate every credentialed
  # job: an exact-branch policy on each, reviewers on production.
  attribute_condition = "assertion.repository_id == '${var.repository_id}' && assertion.repository_owner_id == '${var.repository_owner_id}' && ((${local.deploy_assertion}) || (${local.notification_assertion}))"
  oidc { issuer_uri = "https://token.actions.githubusercontent.com" }
}

resource "google_service_account_iam_member" "github" {
  for_each           = google_service_account.automation
  service_account_id = each.value.name
  role               = "roles/iam.workloadIdentityUser"
  member             = "principalSet://iam.googleapis.com/${google_iam_workload_identity_pool.github.name}/attribute.workflow_role/${each.key == "notification-reader" ? "notification-reader" : "deploy"}"
}

resource "google_project_iam_member" "deploy" {
  for_each = toset([
    "roles/compute.instanceAdmin.v1", "roles/compute.networkAdmin",
    "roles/container.admin", "roles/iam.serviceAccountAdmin",
    "roles/secretmanager.admin", "roles/artifactregistry.reader",
    "roles/serviceusage.serviceUsageConsumer",
    "roles/run.admin", "roles/cloudkms.admin",
  ])
  project = var.project_id
  role    = each.value
  member  = "serviceAccount:${google_service_account.automation["deploy"].email}"
}

// Environment automation grants the shared GKE node identity its node role and
// can modify no other project-level binding.
resource "google_project_iam_member" "deploy_node_roles" {
  project = var.project_id
  role    = "roles/resourcemanager.projectIamAdmin"
  member  = "serviceAccount:${google_service_account.automation["deploy"].email}"
  condition {
    title      = "only-gke-node-service-account-role"
    expression = "api.getAttribute('iam.googleapis.com/modifiedGrantsByRole', []).hasOnly(['roles/container.defaultNodeServiceAccount'])"
  }
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

// The notification workflow never reads lifecycle/terraform state or application
// secrets. Deploy creates nonsecret immutable receipts; notifier reads receipts
// and owns only its delivery claim/ack namespace.
resource "google_storage_bucket" "notifications" {
  name                        = var.notification_bucket_name
  location                    = "US-CENTRAL1"
  uniform_bucket_level_access = true
  public_access_prevention    = "enforced"
  force_destroy               = false
  lifecycle { prevent_destroy = true }
  depends_on = [google_project_service.required]
}
resource "google_storage_bucket_iam_member" "notification_receipt_writer" {
  bucket = google_storage_bucket.notifications.name
  role   = "roles/storage.objectAdmin"
  member = "serviceAccount:${google_service_account.automation["deploy"].email}"
  condition {
    title      = "only-deployment-receipts"
    expression = "resource.name.startsWith('projects/_/buckets/${google_storage_bucket.notifications.name}/objects/notifications/')"
  }
}
resource "google_storage_bucket_iam_member" "notification_reader" {
  bucket = google_storage_bucket.notifications.name
  role   = "roles/storage.objectViewer"
  member = "serviceAccount:${google_service_account.automation["notification-reader"].email}"
}
resource "google_storage_bucket_iam_member" "notification_delivery" {
  bucket = google_storage_bucket.notifications.name
  role   = "roles/storage.objectAdmin"
  member = "serviceAccount:${google_service_account.automation["notification-reader"].email}"
  condition {
    title      = "only-notification-delivery-claims"
    expression = "resource.name.startsWith('projects/_/buckets/${google_storage_bucket.notifications.name}/objects/notification-delivery/')"
  }
}

// Production promotes already-validated staging artifacts. Its deployment
// identity can GET sanitized evidence but cannot list, replace or delete it.
resource "google_storage_bucket_iam_member" "staging_promotion_reader" {
  count  = var.deployment_tier == "production" ? 1 : 0
  bucket = var.staging_release_bucket
  role   = "roles/storage.objectViewer"
  member = "serviceAccount:${google_service_account.automation["deploy"].email}"
  condition {
    title      = "only-staging-promotion-evidence"
    expression = "resource.name.startsWith('projects/_/buckets/${var.staging_release_bucket}/objects/deployments/develop/')"
  }
}

// Human bootstrap owns the retained production node identity and its only
// cross-project grant. Deployment cannot modify staging-registry IAM.
resource "google_service_account" "production_nodes" {
  count        = var.deployment_tier == "production" ? 1 : 0
  account_id   = "aidash-gke-nodes"
  display_name = "Aidash production GKE nodes"
  lifecycle { prevent_destroy = true }
  depends_on = [google_project_service.required]
}
resource "google_artifact_registry_repository_iam_member" "production_nodes_staging" {
  count      = var.deployment_tier == "production" ? 1 : 0
  project    = var.nonproduction_project_id
  location   = "us-central1"
  repository = "aidash"
  role       = "roles/artifactregistry.reader"
  member     = "serviceAccount:${google_service_account.production_nodes[0].email}"
}
