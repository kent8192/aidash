variable "project_id" { type = string }
variable "cloudflare_zone_id" { type = string }
variable "release_bucket" { type = string }
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
  description = "Controller-generated desired state. Absence retires that environment and its private disks."
  type = map(object({
    kind          = string
    generation    = number
    incarnation   = string
    running       = bool
    published     = bool
    spot          = bool
    bundle_object = string
    bundle_sha256 = string
    release_sha   = string
    vm_present    = optional(bool, true)
    machine_type  = optional(string, "e2-standard-4")
    boot_disk_gib = optional(number, 10)
    data_disk_gib = optional(number, 20)
  }))
  default = {}
  validation {
    condition = alltrue([for id, e in var.environments :
      (id == "develop" && e.kind == "develop" || id == "test" && e.kind == "test" || can(regex("^pr-[1-9][0-9]*$", id)) && e.kind == "pr") &&
      can(regex("^[a-f0-9]{12}$", e.incarnation)) && e.generation >= 1 &&
      can(regex("^[a-f0-9]{64}$", e.bundle_sha256)) &&
      can(regex("^[a-f0-9]{40}$", e.release_sha)) &&
      can(regex("^bundles/[a-f0-9]{64}\\.tar\\.gz$", e.bundle_object)) &&
      (!e.published || e.running) && (!e.running || e.vm_present) && (e.kind != "develop" || !e.spot) &&
      e.boot_disk_gib >= 10 && e.data_disk_gib >= 20
    ])
    error_message = "Only retained nonproduction environments with immutable bundles are permitted; production and Spot develop are forbidden."
  }
  validation {
    condition = alltrue([for kind in ["develop", "test", "pr"] :
      length([for e in values(var.environments) : e if e.kind == kind && e.running]) <= 1
    ])
    error_message = "At most one environment of each kind may run. Stop the active PR before starting another."
  }
}
