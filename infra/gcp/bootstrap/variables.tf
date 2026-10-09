variable "project_id" {
  type        = string
  description = "Existing, billing-enabled GCP project. Never infer it from the local gcloud default."
  validation {
    condition     = can(regex("^[a-z][a-z0-9-]{4,28}[a-z0-9]$", var.project_id))
    error_message = "Use an explicit GCP project ID."
  }
}

variable "repository" {
  type    = string
  default = "kent8192/aidash"
}

variable "repository_id" {
  type    = string
  default = "1378229915"
}

variable "repository_owner_id" {
  type    = string
  default = "51869472"
}

variable "trusted_branch" {
  type    = string
  default = "main"
}

variable "state_bucket_name" {
  type        = string
  description = "Globally unique private Terraform/lifecycle state bucket name."
}

variable "release_bucket_name" {
  type        = string
  description = "Globally unique private immutable release bundle bucket name."
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
variable "byok_broker_environments" {
  description = "Retained BYOK broker environment IDs. Human bootstrap owns identities, signing keys and read grants independently of automation enablement; never PR previews."
  type        = set(string)
  default     = []
  validation {
    condition     = alltrue([for id in var.byok_broker_environments : can(regex("^[a-z][a-z0-9-]{0,15}$", id)) && !startswith(id, "pr-")])
    error_message = "Broker environments must use canonical IDs of at most 16 characters and exclude PR previews."
  }
  validation {
    condition     = alltrue([for left in var.byok_broker_environments : alltrue([for right in var.byok_broker_environments : left == right || !startswith("aidash-${right}-cred-", "aidash-${left}-cred-")])])
    error_message = "Broker environment credential prefixes must not overlap."
  }
}
variable "broker_signing_region" {
  description = "Permanent KMS key-ring location; match the broker module region."
  type        = string
  default     = "us-central1"
}
