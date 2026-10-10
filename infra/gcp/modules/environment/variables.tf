variable "project_id" { type = string }
variable "environment_id" { type = string }
variable "hostname" { type = string }
variable "node_service_account" {
  description = "Shared node identity; it holds no Aidash permissions."
  type        = string
}
variable "cluster" {
  description = "Shared GKE cluster that hosts this environment's node pool and Workload Identity principals."
  type = object({
    name          = string
    location      = string
    workload_pool = string
  })
}
variable "environment" {
  type = object({
    kind         = string
    incarnation  = string
    running      = bool
    spot         = bool
    nodes        = number
    machine_type = string
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
variable "broker" {
  description = "Non-secret worker settings from the environment's Credential Broker."
  type = object({
    endpoint = string
    issuer   = string
    audience = string
    kid      = string
  })
  default = null
}
