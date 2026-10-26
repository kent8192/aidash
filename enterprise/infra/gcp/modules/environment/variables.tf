variable "project_id" { type = string }
variable "deployment_tier" {
  type    = string
  default = "nonproduction"
  validation {
    condition     = contains(["nonproduction", "production"], var.deployment_tier)
    error_message = "Deployment tier must be nonproduction or production."
  }
}
variable "environment_id" { type = string }
variable "hostname" { type = string }
variable "node_service_account" {
  description = "Shared node identity; it holds no Aidash permissions."
  type        = string
}
variable "cluster" {
  description = "Shared GKE cluster that hosts this environment's node pool and Workload Identity principals."
  type = object({
    name           = string
    location       = string
    workload_pool  = string
    node_locations = optional(list(string))
    // Zones whose pool stays at zero nodes until the first zone is lost.
    standby_zones = optional(list(string), [])
  })
}
variable "environment" {
  type = object({
    kind               = string
    incarnation        = string
    running            = bool
    spot               = bool
    nodes              = number
    machine_type       = string
    auth_helper        = optional(string, "firebase")
    recovery_protected = optional(bool, false)
  })
  validation {
    condition     = var.deployment_tier == "production" ? (var.environment_id == "production" && var.environment.kind == "production" && var.environment.running && !var.environment.spot) : var.environment.kind != "production" && var.environment_id != "production"
    error_message = "Production must be isolated in its own tier, run continuously and use normal nodes."
  }
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
