variable "project_id" { type = string }
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
variable "enabled" {
  type    = bool
  default = false
  validation {
    condition     = !var.enabled || var.environment_kind != "pr"
    error_message = "PR preview environments must never deploy a Credential Broker."
  }
}
variable "secret_prefix" {
  description = "The Provider Credential prefix output by #136, without incarnation."
  type        = string
  validation {
    condition     = var.secret_prefix == "aidash-${var.environment_id}-cred-"
    error_message = "The BYOK secret prefix must match this environment."
  }
}
variable "runtime_service_account" {
  description = "Optional worker signer for standalone composition. Roots that feed broker settings back to the worker bind it after both modules exist."
  type        = string
  default     = null
  validation {
    condition     = !var.enabled || !var.bind_runtime_signer || var.runtime_service_account != null
    error_message = "Standalone enabled brokers require a worker runtime signer account."
  }
}
variable "bind_runtime_signer" {
  description = "Bind the worker signer inside this module; false when the composing root owns that binding."
  type        = bool
  default     = true
}
variable "deploy_service_account" { type = string }
variable "broker_service_account_email" {
  description = "broker_service_accounts[environment_id] from human-run bootstrap; #137 owns the SA and BYOK read grant."
  type        = string
  validation {
    condition     = var.broker_service_account_email == "aidash-${var.environment_id}-broker@${var.project_id}.iam.gserviceaccount.com"
    error_message = "Use this environment's bootstrap-created broker SA in the application project."
  }
}
variable "image" {
  type = string
  validation {
    condition     = can(regex("@sha256:[a-f0-9]{64}$", var.image))
    error_message = "The broker image must be pinned by digest."
  }
}
variable "region" {
  type    = string
  default = "us-central1"
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
  type    = number
  default = 3600
  validation {
    condition     = var.timeout_secs >= 1 && var.timeout_secs <= 3600 && floor(var.timeout_secs) == var.timeout_secs
    error_message = "Broker inference deadlines must be within 1..3600 seconds."
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
    condition     = var.burst >= 1 && floor(var.burst) == var.burst
    error_message = "The Provider Credential burst must be a positive integer."
  }
}
