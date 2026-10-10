variable "project_id" { type = string }
variable "cloudflare_zone_id" { type = string }
variable "deploy_service_account" { type = string }
variable "domain" {
  type    = string
  default = "aidash.run"
  validation {
    condition     = can(regex("^[a-z0-9][a-z0-9.-]+[a-z0-9]$", var.domain))
    error_message = "Use a DNS name, without a protocol or shell characters."
  }
}

variable "environments" {
  description = "Controller-generated desired state. Absence retires that environment's node pool, identities and runtime secret."
  type = map(object({
    kind         = string
    generation   = number
    incarnation  = string
    running      = bool
    published    = bool
    spot         = bool
    address      = optional(string)
    nodes        = optional(number, 1)
    machine_type = optional(string, "n2-standard-4")
    release_sha  = optional(string)
  }))
  default = {}
  validation {
    condition = alltrue([for id, e in var.environments :
      (id == "develop" && e.kind == "develop" || id == "test" && e.kind == "test" || can(regex("^pr-[1-9][0-9]*$", id)) && e.kind == "pr") &&
      can(regex("^[a-f0-9]{12}$", e.incarnation)) && e.generation >= 1 &&
      (e.release_sha == null || can(regex("^[a-f0-9]{40}$", e.release_sha))) &&
      (e.address == null || can(cidrnetmask("${e.address}/32"))) &&
      e.nodes >= 1 && floor(e.nodes) == e.nodes && startswith(e.machine_type, "n2-") &&
      (!e.published || e.running) && (e.kind != "develop" || !e.spot)
    ])
    error_message = "Only nonproduction N2 environments are permitted; production and Spot develop are forbidden."
  }
  validation {
    condition = alltrue([for kind in ["develop", "test", "pr"] :
      length([for e in values(var.environments) : e if e.kind == kind && e.running]) <= 1
    ])
    error_message = "At most one environment of each kind may run. Stop the active PR before starting another."
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
