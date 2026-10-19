variable "project_id" { type = string }
variable "gke_backup_enabled" {
  description = "Install the production Backup for GKE agent after the bootstrap API preparation, independently of second-phase backup plan creation."
  type        = bool
  default     = false
  validation {
    condition     = !var.gke_backup_enabled || var.deployment_tier == "production"
    error_message = "Backup for GKE agent activation is limited to isolated production."
  }
}

variable "deployment_tier" {
  description = "A production stack owns only production and uses its own project/backend."
  type        = string
  default     = "nonproduction"
  validation {
    condition     = contains(["nonproduction", "production"], var.deployment_tier)
    error_message = "Deployment tier must be nonproduction or production."
  }
}
variable "nonproduction_project_id" {
  description = "Explicit nonproduction project used to prove production isolation."
  type        = string
  default     = ""
  validation {
    condition     = var.deployment_tier != "production" || (can(regex("^[a-z][a-z0-9-]{4,28}[a-z0-9]$", var.nonproduction_project_id)) && var.project_id != var.nonproduction_project_id)
    error_message = "Production requires a distinct project from the explicit nonproduction project."
  }
}
variable "state_bucket" {
  description = "Actual backend bucket, supplied and checked by the trusted controller."
  type        = string
  default     = ""
}
variable "nonproduction_state_bucket" {
  description = "Explicit nonproduction backend bucket used to prove production isolation."
  type        = string
  default     = ""
  validation {
    condition     = var.deployment_tier != "production" || (var.state_bucket != "" && var.nonproduction_state_bucket != "" && var.state_bucket != var.nonproduction_state_bucket)
    error_message = "Production requires its own explicit backend bucket, distinct from nonproduction."
  }
}
variable "production_hostname" {
  description = "Fixed production_origin hostname; random hosts are only for nonproduction."
  type        = string
  default     = "aidash.run"
  validation {
    condition     = can(regex("^[a-z0-9][a-z0-9.-]+[a-z0-9]$", var.production_hostname)) && !endswith(var.production_hostname, ".preview.aidash.run")
    error_message = "Production requires a fixed DNS hostname outside the preview namespace."
  }
}
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
    kind           = string
    generation     = number
    incarnation    = string
    hostname_token = optional(string)
    # Trusted recovery marks only persisted pre-randomization state. New
    # controller deployments always replace this flag with a random token.
    legacy_hostname = optional(bool, false)
    running         = bool
    published       = bool
    spot            = bool
    address         = optional(string)
    nodes           = optional(number, 1)
    machine_type    = optional(string, "n2-standard-4")
    release_sha     = optional(string)
    # Set to "public_origin" only with a release deployment, so the SAML ACS
    # switches in the same apply that installs the edge's sign-in helper proxy.
    auth_helper = optional(string, "firebase")
  }))
  default = {}
  validation {
    condition = alltrue([for id, e in var.environments :
      (var.deployment_tier == "production" ? id == "production" && e.kind == "production" :
      (id == "develop" && e.kind == "develop" || id == "test" && e.kind == "test" || can(regex("^pr-[1-9][0-9]*$", id)) && e.kind == "pr")) &&
      (contains(["develop", "pr"], e.kind) ?
        (e.legacy_hostname ? e.hostname_token == null : can(regex("^[a-f0-9]{32}$", e.hostname_token))) :
      !e.legacy_hostname && e.hostname_token == null) &&
      can(regex("^[a-f0-9]{12}$", e.incarnation)) && e.generation >= 1 &&
      (e.release_sha == null || can(regex("^[a-f0-9]{40}$", e.release_sha))) &&
      (e.address == null || can(cidrnetmask("${e.address}/32"))) &&
      e.nodes >= 1 && floor(e.nodes) == e.nodes && startswith(e.machine_type, "n2-") &&
      (!e.published || e.running) && (!contains(["develop", "production"], e.kind) || !e.spot) &&
      (e.kind != "production" || e.running)
    ])
    error_message = "Use tier-matching N2 environments, random preview tokens or explicit trusted legacy state, and normal always-running production nodes."
  }
  validation {
    # Losing us-central1-a makes the system node and every Environment node
    # NotReady at once. The cluster autoscaler, which grows the standby pools,
    # halts only when more than three nodes and more than 45% are NotReady
    # (upstream --ok-total-unready-count=3); GKE documents only the 45%, so the
    # zone failover drill confirms the allowance (ADR 0067).
    condition     = alltrue([for e in values(var.environments) : e.kind != "production" || e.nodes <= 2])
    error_message = "Production runs at most two Environment nodes, so a lost zone leaves at most three NotReady nodes and the autoscaler can still fill the standby zone (ADR 0067)."
  }
  validation {
    condition     = length(distinct(compact([for e in values(var.environments) : e.hostname_token]))) == length([for e in values(var.environments) : e if e.hostname_token != null])
    error_message = "Preview tokens must be distinct across managed environments."
  }
  validation {
    condition     = alltrue([for e in values(var.environments) : contains(["firebase", "public_origin"], e.auth_helper)])
    error_message = "The sign-in helper is either Firebase's default domain or the environment's public origin."
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

variable "staging_project_id" {
  description = "Production pulls the exact tested image digests from the nonproduction Artifact Registry."
  type        = string
  default     = ""
  validation {
    condition     = var.deployment_tier == "production" ? (var.staging_project_id == var.nonproduction_project_id && var.staging_project_id != var.project_id) : var.staging_project_id == ""
    error_message = "Only production may pull staging artifacts, from the explicit distinct nonproduction project."
  }
}
