// Provider Credential Key Material lives in a separate, dedicated BYOK project.
// Create is checked on the project and cannot be limited by a Secret name.
// Manage is checked on Secret resources, whose IAM names use project NUMBER.
data "google_project" "byok" {
  project_id = var.byok_project_id
}
locals {
  provider_credential_secret_prefix = "aidash-${var.environment_id}-cred-"
}
// Bootstrap defines these fixed roles once per deployment. Automation may
// bind them but has no iam.roles.* permission in the BYOK project.
resource "google_project_iam_member" "provider_credential_create" {
  project = var.byok_project_id
  role    = "projects/${var.byok_project_id}/roles/aidashByokCreate"
  member  = "serviceAccount:${google_service_account.runtime.email}"
}
resource "google_project_iam_member" "provider_credential_manage" {
  project = var.byok_project_id
  role    = "projects/${var.byok_project_id}/roles/aidashByokManage"
  member  = "serviceAccount:${google_service_account.runtime.email}"
  condition {
    title      = "own-environment-provider-credentials"
    expression = "resource.name.startsWith('projects/${data.google_project.byok.number}/secrets/${local.provider_credential_secret_prefix}')"
  }
}
output "byok_project_id" { value = var.byok_project_id }
output "secret_prefix" { value = local.provider_credential_secret_prefix }
