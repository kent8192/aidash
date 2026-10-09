variable "gcip_enabled" {
  type    = bool
  default = false
}
variable "environment_domains" {
  description = "The domain values used by environment stacks; GCIP derives allowed callback hosts from these."
  type        = set(string)
  default     = ["aidash.run"]
  validation {
    condition     = alltrue([for domain in var.environment_domains : can(regex("^[a-z0-9][a-z0-9.-]+[a-z0-9]$", domain))])
    error_message = "Use DNS domain names without a scheme or path."
  }
}
resource "google_project_service" "gcip" {
  count              = var.gcip_enabled ? 1 : 0
  project            = var.project_id
  service            = "identitytoolkit.googleapis.com"
  disable_on_destroy = false
}
resource "google_identity_platform_config" "aidash" {
  count   = var.gcip_enabled ? 1 : 0
  project = var.project_id
  multi_tenant { allow_tenants = true }
  client {}
  authorized_domains = sort(distinct(concat(["${var.project_id}.firebaseapp.com"], flatten([
    for domain in var.environment_domains : [for environment in ["develop", "preview", "test"] : "${environment}.${domain}"]
  ]))))
  depends_on = [google_project_service.gcip]
  lifecycle { prevent_destroy = true }
}
# The trusted controller creates/configures tenants and reconciles their IAM.
# Runtime principals receive a read role on tenant resources through REST only.
resource "google_project_iam_member" "gcip_deploy" {
  count   = var.gcip_enabled ? 1 : 0
  project = var.project_id
  role    = "roles/identityplatform.admin"
  member  = "serviceAccount:${google_service_account.automation["deploy"].email}"
}
output "gcip_web_api_key" {
  value     = var.gcip_enabled ? google_identity_platform_config.aidash[0].client[0].api_key : null
  sensitive = true
}
