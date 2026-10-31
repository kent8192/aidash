# SPDX-FileCopyrightText: 2026 kent8192
# SPDX-License-Identifier: BUSL-1.1
# See enterprise/LICENSE.

// Terraform owns metadata only. Certificate/key JSON versions are added by the
// trusted renewal workflow and must never enter Terraform configuration/state.
variable "preview_tls_secret_id" {
  description = "Retained nonproduction wildcard TLS Secret metadata. Import an existing Secret before applying."
  type        = string
  default     = "aidash-preview-wildcard-tls"
  validation {
    condition     = can(regex("^[A-Za-z0-9_-]{1,255}$", var.preview_tls_secret_id))
    error_message = "Use the explicit preview TLS Secret ID, never its payload."
  }
}

variable "preview_acme_state_secret_id" {
  description = "Separate retained Secret metadata for Certbot ACME account registration and private key."
  type        = string
  default     = "aidash-preview-acme-state"
  validation {
    condition     = can(regex("^[A-Za-z0-9_-]{1,255}$", var.preview_acme_state_secret_id)) && var.preview_acme_state_secret_id != var.preview_tls_secret_id
    error_message = "ACME account state requires its own explicit Secret ID distinct from the TLS Secret."
  }
}

locals {
  tls_renewal_enabled      = var.deployment_tier == "nonproduction"
  tls_renewal_workflow_ref = "${var.repository}/.github/workflows/deployment-certificate.yml@refs/heads/${var.automation_branch}"
}

resource "google_secret_manager_secret" "preview_tls" {
  count     = local.tls_renewal_enabled ? 1 : 0
  project   = var.project_id
  secret_id = var.preview_tls_secret_id
  replication {
    auto {}
  }
  lifecycle {
    prevent_destroy = true
    precondition {
      condition     = var.delivery_configuration == null ? true : var.preview_tls_secret_id == var.delivery_configuration.preview_tls_secret
      error_message = "Delivery configuration must point to the bootstrap-managed preview TLS Secret."
    }
  }
  depends_on = [google_project_service.required]
}

resource "google_secret_manager_secret" "preview_acme_state" {
  count     = local.tls_renewal_enabled ? 1 : 0
  project   = var.project_id
  secret_id = var.preview_acme_state_secret_id
  replication {
    auto {}
  }
  lifecycle { prevent_destroy = true }
  depends_on = [google_project_service.required]
}

resource "google_service_account" "tls_renewer" {
  count        = local.tls_renewal_enabled ? 1 : 0
  account_id   = "aidash-tls-renewer"
  display_name = "Aidash protected preview wildcard certificate renewal"
  depends_on   = [google_project_service.required]
}

resource "google_iam_workload_identity_pool_provider" "tls_renewal" {
  count                              = local.tls_renewal_enabled ? 1 : 0
  workload_identity_pool_id          = google_iam_workload_identity_pool.github.workload_identity_pool_id
  workload_identity_pool_provider_id = "github-certificate-renewal"
  attribute_mapping = {
    "google.subject"          = "assertion.sub"
    "attribute.workflow_role" = "'tls-renewer'"
  }
  attribute_condition = "assertion.repository_id == '${var.repository_id}' && assertion.repository_owner_id == '${var.repository_owner_id}' && assertion.workflow_ref == '${local.tls_renewal_workflow_ref}' && assertion.ref == 'refs/heads/${var.automation_branch}' && assertion.environment == 'certificate-renewal'"
  oidc { issuer_uri = "https://token.actions.githubusercontent.com" }
}

resource "google_service_account_iam_member" "tls_renewal_github" {
  count              = local.tls_renewal_enabled ? 1 : 0
  service_account_id = google_service_account.tls_renewer[0].name
  role               = "roles/iam.workloadIdentityUser"
  member             = "principalSet://iam.googleapis.com/${google_iam_workload_identity_pool.github.name}/attribute.workflow_role/tls-renewer"
}

// List only this Secret's version metadata to distinguish an empty Secret from
// revoked latest/permission errors. No secret administration, destroy or disable.
resource "google_project_iam_custom_role" "tls_renewer" {
  count       = local.tls_renewal_enabled ? 1 : 0
  project     = var.project_id
  role_id     = "aidashPreviewTlsRenewer"
  title       = "Aidash preview TLS version reader and adder"
  description = "Read/list/add versions only; bound exclusively to preview TLS Secret."
  permissions = ["secretmanager.versions.access", "secretmanager.versions.add", "secretmanager.versions.list"]
}

resource "google_secret_manager_secret_iam_member" "tls_renewer" {
  for_each   = local.tls_renewal_enabled ? toset([var.preview_tls_secret_id, var.preview_acme_state_secret_id]) : toset([])
  project    = var.project_id
  secret_id  = each.key
  role       = google_project_iam_custom_role.tls_renewer[0].name
  member     = "serviceAccount:${google_service_account.tls_renewer[0].email}"
  depends_on = [google_secret_manager_secret.preview_tls, google_secret_manager_secret.preview_acme_state]
}

output "preview_tls_renewal" {
  description = "Nonsecret metadata for the protected certificate-renewal workflow; null in production."
  value = local.tls_renewal_enabled ? {
    secret_id                  = google_secret_manager_secret.preview_tls[0].secret_id
    acme_state_secret_id       = google_secret_manager_secret.preview_acme_state[0].secret_id
    service_account            = google_service_account.tls_renewer[0].email
    workload_identity_provider = google_iam_workload_identity_pool_provider.tls_renewal[0].name
  } : null
}
