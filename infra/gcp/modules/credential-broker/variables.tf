variable "project_id" { type = string }
variable "deployment_tier" {
  type    = string
  default = "nonproduction"
  validation {
    condition     = contains(["nonproduction", "production"], var.deployment_tier)
    error_message = "Deployment tier must be nonproduction or production."
  }
}
variable "byok_project_id" {
  type = string
  validation {
    condition     = var.byok_project_id != var.project_id && length(var.byok_project_id) > 0
    error_message = "Provider Credentials require a dedicated BYOK project."
  }
}
variable "environment_id" {
  type = string
  validation {
    condition     = can(regex("^[a-z][a-z0-9-]{0,15}$", var.environment_id))
    error_message = "Use a canonical environment ID of at most 16 characters."
  }
}
variable "environment_kind" { type = string }
variable "role" {
  description = "Which Secret Store namespace this service reads: Key Material (-cred-) or Tenant Secrets (-tsec-)."
  type        = string
  default     = "key-material"
  validation {
    condition     = contains(["key-material", "tenant-secrets"], var.role)
    error_message = "A broker role is key-material or tenant-secrets."
  }
}
variable "enabled" {
  type    = bool
  default = false
  validation {
    condition     = !var.enabled || var.environment_kind != "pr"
    error_message = "PR preview environments must never deploy a Credential Broker."
  }
  validation {
    condition     = !var.enabled || (var.deployment_tier == "production" ? var.environment_id == "production" && var.environment_kind == "production" : var.environment_id != "production" && var.environment_kind != "production")
    error_message = "Production brokers require the dedicated production deployment tier."
  }
}
variable "secret_prefix" {
  description = "The Provider Credential (-cred-) or Tenant Secret (-tsec-) prefix for this environment."
  type        = string
  validation {
    condition     = var.secret_prefix == (var.role == "tenant-secrets" ? "aidash-${var.environment_id}-tsec-" : "aidash-${var.environment_id}-cred-")
    error_message = "The secret prefix must match this environment and the broker role."
  }
}
variable "runtime_service_account" {
  description = "Optional runtime signer for standalone composition. Roots that feed broker settings back to the worker or server bind it after both modules exist."
  type        = string
  default     = null
  validation {
    condition     = !var.enabled || !var.bind_runtime_signer || var.runtime_service_account != null
    error_message = "Standalone enabled brokers require a runtime signer account."
  }
}
variable "bind_runtime_signer" {
  description = "Bind the worker signer inside this module; false when the composing root owns that binding."
  type        = bool
  default     = true
}
variable "broker_service_account_email" {
  description = "broker_service_accounts or tenant_secret_broker_service_accounts[environment_id] from human-run bootstrap; #137 owns the SAs and BYOK read grants."
  type        = string
  validation {
    condition     = var.broker_service_account_email == "${var.role == "tenant-secrets" ? "aidash-${var.environment_id}-tsec" : "aidash-${var.environment_id}-broker"}@${var.project_id}.iam.gserviceaccount.com"
    error_message = "Use this environment's bootstrap-created broker SA for the selected role in the application project."
  }
}
variable "image" {
  type = string
  validation {
    condition     = can(regex("^us-central1-docker\\.pkg\\.dev/${var.project_id}/aidash/broker@sha256:[a-f0-9]{64}$", var.image))
    error_message = "The broker image must be this project's private Artifact Registry aidash/broker image pinned by digest."
  }
}
variable "region" {
  type    = string
  default = "us-central1"
}
variable "signing_key_id" {
  description = "Stable broker_signing_keys or tenant_secret_signing_keys entry from human-run bootstrap. This module never owns or destroys signing keys."
  type        = string
  default     = ""
  validation {
    condition     = !var.enabled || var.signing_key_id == "projects/${var.project_id}/locations/${var.region}/keyRings/aidash-${var.environment_id}-capability/cryptoKeys/${var.role == "tenant-secrets" ? "tenant-secret" : "capability"}"
    error_message = "Use this environment's bootstrap signing key for the broker role in the application project and broker region."
  }
}
variable "secondary_signing_key_id" {
  description = "The worker's verification key, injected into the Tenant Secret broker so it accepts both issuers (ADR 0005 amendment)."
  type        = string
  default     = ""
  validation {
    condition     = !var.enabled || var.role != "tenant-secrets" || var.secondary_signing_key_id != ""
    error_message = "An enabled Tenant Secret broker must accept the worker issuer's public key."
  }
  validation {
    condition     = var.secondary_signing_key_id == "" || (var.role == "tenant-secrets" && var.secondary_signing_key_id == "projects/${var.project_id}/locations/${var.region}/keyRings/aidash-${var.environment_id}-capability/cryptoKeys/capability")
    error_message = "Only the Tenant Secret broker may verify with the worker key."
  }
}
variable "secondary_verification_versions" {
  description = "Worker key versions to publish to the Tenant Secret broker during rotation."
  type        = set(string)
  default     = ["1"]
  validation {
    condition     = alltrue([for v in var.secondary_verification_versions : can(regex("^[1-9][0-9]*$", v))])
    error_message = "Secondary verification versions must be positive integers."
  }
}
variable "secondary_issuer" {
  description = "Issuer carried by tokens signed with the secondary (worker) key; required for the Tenant Secret broker."
  type        = string
  default     = ""
  validation {
    condition     = !var.enabled || var.role != "tenant-secrets" || var.secondary_issuer != ""
    error_message = "An enabled Tenant Secret broker must name the worker issuer."
  }
  validation {
    condition     = var.secondary_issuer == "" || var.role == "tenant-secrets"
    error_message = "Only the Tenant Secret broker may configure a secondary issuer."
  }
}
variable "issuer" {
  type    = string
  default = "aidash-worker"
}
variable "signing_version" {
  type    = string
  default = "1"
}
variable "verification_versions" {
  description = "Inject current and overlapping previous public keys during rotation."
  type        = set(string)
  default     = ["1"]
  validation {
    condition     = contains(var.verification_versions, var.signing_version) && alltrue([for v in var.verification_versions : can(regex("^[1-9][0-9]*$", v))])
    error_message = "Verification must include the pinned numeric signing version."
  }
}
variable "ingress" {
  type    = string
  default = "INGRESS_TRAFFIC_INTERNAL_ONLY"
  validation {
    condition     = var.ingress == "INGRESS_TRAFFIC_INTERNAL_ONLY"
    error_message = "The Credential Broker must use internal ingress."
  }
}
variable "invoker_iam_disabled" {
  type    = bool
  default = true
  validation {
    condition     = var.invoker_iam_disabled
    error_message = "Capability Tokens are the only broker authentication."
  }
}
variable "min_instances" {
  type    = number
  default = null
  validation {
    condition     = var.min_instances == null || try(var.min_instances >= 0 && floor(var.min_instances) == var.min_instances && var.min_instances <= var.max_instances, false)
    error_message = "Minimum instances must be a nonnegative integer no greater than the maximum."
  }
}
variable "max_instances" {
  type    = number
  default = 10
  validation {
    condition     = var.max_instances >= 1 && floor(var.max_instances) == var.max_instances
    error_message = "Maximum instances must be a positive integer."
  }
}
variable "timeout_secs" {
  description = "Cloud Run request timeout; the broker inference deadline is 30 seconds shorter."
  type        = number
  default     = 3600
  validation {
    condition     = var.timeout_secs >= 31 && var.timeout_secs <= 3600 && floor(var.timeout_secs) == var.timeout_secs
    error_message = "Cloud Run timeouts must be integers within 31..3600 seconds, leaving 30 seconds outside the broker deadline."
  }
}
variable "requests_per_second" {
  type    = number
  default = 5
  validation {
    condition     = var.requests_per_second > 0
    error_message = "The Provider Credential refill rate must be positive."
  }
}
variable "burst" {
  type    = number
  default = 20
  validation {
    condition     = var.burst >= 3 && floor(var.burst) == var.burst
    error_message = "The Provider Credential burst must be an integer of at least three for media discovery plus chat."
  }
}
