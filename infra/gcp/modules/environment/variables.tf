variable "project_id" { type = string }
variable "environment_id" { type = string }
variable "hostname" { type = string }
variable "release_bucket" { type = string }
variable "deploy_service_account" { type = string }
variable "preview_tls_disk" {
  description = "Shared preview certificate store; attached only while this environment runs."
  type        = string
  default     = null
}
variable "environment" {
  type = object({
    kind          = string
    generation    = number
    incarnation   = string
    running       = bool
    published     = bool
    spot          = bool
    bundle_object = string
    bundle_sha256 = string
    release_sha   = string
    vm_present    = bool
    machine_type  = string
    boot_disk_gib = number
    data_disk_gib = number
  })
}

variable "byok_project_id" {
  type        = string
  description = "Optional dedicated Provider Credential project; empty disables BYOK provisioning."
  default     = ""
  validation {
    condition     = var.byok_project_id == "" || (can(regex("^[a-z][a-z0-9-]{4,28}[a-z0-9]$", var.byok_project_id)) && var.byok_project_id != var.project_id)
    error_message = "Use an explicit dedicated BYOK project distinct from the shared environment project."
  }
}
