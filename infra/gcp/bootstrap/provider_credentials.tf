// Existing, billing-enabled project. Bootstrap does not create GCP projects.
resource "google_project_service" "byok_secret_manager" {
  count              = var.byok_project_id != "" ? 1 : 0
  project            = var.byok_project_id
  service            = "secretmanager.googleapis.com"
  disable_on_destroy = false
}
data "google_project" "byok" {
  count      = var.byok_project_id != "" ? 1 : 0
  project_id = var.byok_project_id
}

// Human-run bootstrap owns these fixed roles; deploy cannot change their permissions.
resource "google_project_iam_custom_role" "byok_create" {
  count       = var.byok_project_id != "" ? 1 : 0
  project     = var.byok_project_id
  role_id     = "aidashByokCreate"
  title       = "Aidash Provider Credential creation"
  permissions = ["secretmanager.secrets.create"]
}
resource "google_project_iam_custom_role" "byok_manage" {
  count   = var.byok_project_id != "" ? 1 : 0
  project = var.byok_project_id
  role_id = "aidashByokManage"
  title   = "Aidash Provider Credential management"
  permissions = [
    "secretmanager.secrets.get", "secretmanager.secrets.delete",
    "secretmanager.versions.add", "secretmanager.versions.disable",
    "secretmanager.versions.destroy", "secretmanager.versions.get",
    "secretmanager.versions.list",
  ]
}
// Read role definition only; #137's human-run bootstrap owns the broker identity and grants.
resource "google_project_iam_custom_role" "byok_broker_read" {
  count   = var.byok_project_id != "" ? 1 : 0
  project = var.byok_project_id
  role_id = "aidashByokBrokerRead"
  title   = "Aidash Provider Credential broker read"
  permissions = [
    "secretmanager.versions.access", "secretmanager.versions.get",
    "secretmanager.secrets.get",
  ]
}

// Resource Manager honours modifiedGrantsByRole for projects.setIamPolicy:
// https://docs.cloud.google.com/iam/docs/setting-limits-on-granting-roles
// Only runtime roles can be granted/revoked, including their conditions. Neither
// has payload or IAM permissions, and deploy cannot modify the roles themselves.
resource "google_project_iam_custom_role" "byok_deploy" {
  count   = var.byok_project_id != "" ? 1 : 0
  project = var.byok_project_id
  role_id = "aidashByokDeployment"
  title   = "Aidash BYOK deployment IAM management"
  permissions = [
    "resourcemanager.projects.get", "resourcemanager.projects.getIamPolicy",
    "resourcemanager.projects.setIamPolicy",
  ]
}
resource "google_project_iam_member" "byok_deploy" {
  count   = var.byok_project_id != "" ? 1 : 0
  project = var.byok_project_id
  role    = google_project_iam_custom_role.byok_deploy[0].name
  member  = "serviceAccount:${google_service_account.automation["deploy"].email}"
  condition {
    title      = "only-provider-credential-runtime-roles"
    expression = "api.getAttribute('iam.googleapis.com/modifiedGrantsByRole', []).hasOnly(['projects/${var.byok_project_id}/roles/aidashByokCreate', 'projects/${var.byok_project_id}/roles/aidashByokManage'])"
  }
}

// One audit configuration per deployment; all environments share this project.
resource "google_project_iam_audit_config" "byok_secret_manager" {
  count   = var.byok_project_id != "" ? 1 : 0
  project = var.byok_project_id
  service = "secretmanager.googleapis.com"
  audit_log_config { log_type = "DATA_READ" }
  audit_log_config { log_type = "DATA_WRITE" }
}
output "byok_project_id" { value = var.byok_project_id }
