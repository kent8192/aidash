variable "repository" {
  type    = string
  default = "kent8192/aidash"
  validation {
    condition     = var.repository == "kent8192/aidash"
    error_message = "This delivery stack manages the existing kent8192/aidash repository only."
  }
}

variable "automation_branch" {
  description = "Exact protected default/execution branch; trusted application code remains on main."
  type        = string
  default     = "develop/0.1.0"
  validation {
    condition     = can(regex("^develop/[0-9]+\\.[0-9]+\\.[0-9]+$", var.automation_branch))
    error_message = "Use an exact develop/x.y.z branch without wildcard patterns."
  }
}

variable "configure_variables" {
  description = "Second phase only: configure full account/bootstrap/App inputs after protected environments exist. Initial protection phase manages only disabled flags."
  type        = bool
  default     = false
}

variable "nonproduction_config_json" {
  description = "Non-secret controller configuration from the isolated nonproduction bootstrap and domain/App inputs."
  type        = string
  default     = null
  validation {
    condition     = var.nonproduction_config_json == null || can(jsondecode(var.nonproduction_config_json))
    error_message = "Nonproduction configuration must be valid JSON; secret values belong in protected GitHub Environments."
  }
}

variable "production_config_json" {
  description = "Non-secret controller configuration from the isolated production bootstrap and domain/App inputs."
  type        = string
  default     = null
  validation {
    condition     = var.production_config_json == null || can(jsondecode(var.production_config_json))
    error_message = "Production configuration must be valid JSON; secret values belong in protected GitHub Environments."
  }
}

variable "nonproduction_bootstrap" {
  default     = null
  description = "Credential-free bootstrap output. Account identifiers are not secret values."
  type = object({
    project_id                   = string
    state_bucket                 = string
    release_bucket               = string
    notification_bucket          = string
    workload_identity_provider   = string
    deploy_service_account       = string
    publish_service_account      = string
    notification_service_account = string
    preview_tls_renewal = optional(object({
      secret_id                  = string
      acme_state_secret_id       = optional(string)
      service_account            = string
      workload_identity_provider = string
    }))
  })
}

variable "production_bootstrap" {
  default     = null
  description = "Credential-free isolated production bootstrap output."
  type = object({
    project_id                   = string
    state_bucket                 = string
    release_bucket               = string
    notification_bucket          = string
    workload_identity_provider   = string
    deploy_service_account       = string
    notification_service_account = string
  })
}

variable "release_app" {
  default     = null
  description = "Existing installed App metadata only; create/install the App and provision its private key outside Terraform."
  type = object({
    id              = number
    client_id       = string
    slug            = string
    installation_id = number
  })
  validation {
    condition = var.release_app == null ? true : (
      var.release_app.id > 0 && floor(var.release_app.id) == var.release_app.id &&
      var.release_app.installation_id > 0 && floor(var.release_app.installation_id) == var.release_app.installation_id &&
      can(regex("^[a-z0-9]+(?:-[a-z0-9]+)*$", var.release_app.slug)) &&
      can(regex("^[A-Za-z0-9_.-]{1,128}$", var.release_app.client_id))
    )
    error_message = "Supply explicit existing App and installation IDs, client ID and App slug, never a private key."
  }
}

variable "production_reviewer_user_ids" {
  description = "Explicit numeric GitHub user IDs with repository read access; GitHub requires one approval from this set."
  type        = set(number)
  validation {
    condition     = length(var.production_reviewer_user_ids) >= 1 && length(var.production_reviewer_user_ids) <= 6 && alltrue([for id in var.production_reviewer_user_ids : id > 0 && floor(id) == id])
    error_message = "Production requires one to six explicit numeric reviewer user IDs."
  }
}

variable "production_prevent_self_review" {
  description = "Prevent the workflow actor from approving its own production deployment."
  type        = bool
  default     = true
}

variable "require_staging_check" {
  description = "Opt-in after privileged CD code is on main. GitHub cannot restrict a base-branch required check to release/* PR heads."
  type        = bool
  default     = false
}

variable "bootstrap_code_installed" {
  description = "Explicit rollout acknowledgement that trusted workflows/controller exist on protected main and the execution branch."
  type        = bool
  default     = false
}

variable "activation_validated" {
  description = "Explicit controlled activation acknowledgement after account inputs and relevant live acceptance checks."
  type        = bool
  default     = false
}

variable "enablement" {
  description = "Every workflow starts disabled. Set a specific field only for an explicitly authorized activation."
  type = object({
    nonproduction = optional(bool, false)
    production    = optional(bool, false)
    notifications = optional(bool, false)
    certificates  = optional(bool, false)
  })
  default = {}
}

variable "cloudflare_account_id" {
  description = "Non-secret account identifier for the protected notification Environment; empty leaves it unconfigured."
  type        = string
  default     = ""
  validation {
    condition     = var.cloudflare_account_id == "" || can(regex("^[a-f0-9]{32}$", var.cloudflare_account_id))
    error_message = "Use the explicit 32-character Cloudflare account ID or leave it unset."
  }
}

variable "acme_email" {
  description = "Administrative ACME registration email, not the private deployment-alias forwarding destination."
  type        = string
  default     = ""
  validation {
    condition     = var.acme_email == "" || can(regex("^[^\\s@]+@[^\\s@]+\\.[^\\s@]+$", var.acme_email))
    error_message = "Use an administrative email address without whitespace or leave ACME unset."
  }
}

variable "existing_delivery_ruleset_ids" {
  description = "Import only fixed-name owned narrow delivery-check rulesets, keyed main/develop. Leave unrelated and classic protections unmanaged."
  type        = map(number)
  default     = {}
  validation {
    condition     = length(setsubtract(toset(keys(var.existing_delivery_ruleset_ids)), toset(["main", "develop"]))) == 0 && alltrue([for id in values(var.existing_delivery_ruleset_ids) : id > 0 && floor(id) == id])
    error_message = "Only the owned main/develop delivery-check ruleset IDs may be adopted."
  }
}

variable "existing_environments" {
  description = "Existing owned environment names discovered read-only before plan."
  type        = set(string)
  default     = []
  validation {
    condition     = length(setsubtract(var.existing_environments, toset(["nonproduction", "production", "release-automation", "deployment-notifications", "certificate-renewal"]))) == 0
    error_message = "Adopt only the five delivery environments."
  }
}

variable "existing_deployment_policy_ids" {
  description = "Existing exact branch-only policy IDs, keyed environment. Stop on unrelated/wildcard/tag policies rather than deleting them."
  type        = map(number)
  default     = {}
  validation {
    condition     = length(setsubtract(toset(keys(var.existing_deployment_policy_ids)), var.existing_environments)) == 0 && alltrue([for id in values(var.existing_deployment_policy_ids) : id > 0 && floor(id) == id])
    error_message = "Import positive exact-branch policy IDs only for existing owned environments."
  }
}

variable "existing_repository_variables" {
  description = "Existing names discovered from the fixed managed variable inventory."
  type        = set(string)
  default     = []
}

variable "existing_notification_account_variable" {
  description = "Whether CLOUDFLARE_ACCOUNT_ID already exists in deployment-notifications."
  type        = bool
  default     = false
}
