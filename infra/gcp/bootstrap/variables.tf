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

variable "deploy_environment" {
  type        = string
  default     = "gcp-deploy"
  description = "GitHub Environment for unattended, no-build workflow runs (scheduled reconcile, CI updates). Restrict it to the trusted branch; humans configure it in repository settings."
  validation {
    condition     = can(regex("^[A-Za-z0-9][A-Za-z0-9_-]{0,63}$", var.deploy_environment))
    error_message = "Use a plain GitHub Environment name; it is interpolated into the attribute condition."
  }
}

variable "release_environment" {
  type        = string
  default     = "gcp-deploy-release"
  description = "GitHub Environment for runs that build and publish new images. Humans give it required reviewers in repository settings."
  validation {
    condition     = can(regex("^[A-Za-z0-9][A-Za-z0-9_-]{0,63}$", var.release_environment)) && var.release_environment != var.deploy_environment
    error_message = "Use a plain GitHub Environment name that differs from deploy_environment; it is interpolated into the attribute condition."
  }
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
  description = "Permanent KMS key-ring location; managed broker environments support us-central1."
  type        = string
  default     = "us-central1"
  validation {
    condition     = var.broker_signing_region == "us-central1"
    error_message = "Broker signing region must be us-central1 to match the managed environments root."
  }
}
variable "security_alert_emails" {
  description = "Recipients of Credential Broker misuse alerts (#151). Required once any broker environment is configured."
  type        = set(string)
  default     = []
  validation {
    condition     = alltrue([for email in var.security_alert_emails : can(regex("^[^@\\s]+@[^@\\s]+\\.[^@\\s]+$", email))])
    error_message = "Use plain email addresses."
  }
  validation {
    condition     = var.byok_project_id == "" || length(var.byok_broker_environments) == 0 || length(var.security_alert_emails) > 0
    error_message = "Credential Broker alerts need at least one recipient; a silent alert policy detects nothing."
  }
}
