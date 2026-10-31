# SPDX-FileCopyrightText: 2026 kent8192
# SPDX-License-Identifier: BUSL-1.1
# See enterprise/LICENSE.

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

variable "automation_branch" {
  description = "Protected branch containing the nonproduction controller and notification workflow. Exact ref only."
  type        = string
  default     = "develop/0.1.0"
  validation {
    condition     = can(regex("^[A-Za-z0-9][A-Za-z0-9._/-]*$", var.automation_branch)) && !strcontains(var.automation_branch, "..") && !endswith(var.automation_branch, "/")
    error_message = "Use one explicit protected automation branch without glob characters."
  }
}
variable "deployment_tier" {
  type    = string
  default = "nonproduction"
  validation {
    condition     = contains(["nonproduction", "production"], var.deployment_tier)
    error_message = "Deployment tier must be nonproduction or production."
  }
}
variable "nonproduction_project_id" {
  description = "Production bootstrap must name the nonproduction project to prove separation."
  type        = string
  default     = ""
  validation {
    condition     = var.deployment_tier != "production" || (can(regex("^[a-z][a-z0-9-]{4,28}[a-z0-9]$", var.nonproduction_project_id)) && var.project_id != var.nonproduction_project_id)
    error_message = "Production requires an explicit project distinct from nonproduction."
  }
}
variable "nonproduction_state_bucket" {
  type    = string
  default = ""
  validation {
    condition     = var.deployment_tier != "production" || (var.nonproduction_state_bucket != "" && var.state_bucket_name != var.nonproduction_state_bucket)
    error_message = "Production requires an explicit state bucket distinct from nonproduction."
  }
}
variable "notification_bucket_name" {
  description = "Dedicated private receipt/notification delivery bucket; never Terraform state or application secrets."
  type        = string
  validation {
    condition     = var.notification_bucket_name != "" && var.notification_bucket_name != var.state_bucket_name && var.notification_bucket_name != var.release_bucket_name
    error_message = "Notifications require their own dedicated bucket, separate from Terraform state and release bundles."
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
    condition     = var.deployment_tier == "production" ? alltrue([for id in var.byok_broker_environments : id == "production"]) : !contains(var.byok_broker_environments, "production")
    error_message = "Only isolated production bootstrap may create the production broker, and it cannot create nonproduction brokers."
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

variable "staging_release_bucket" {
  description = "Nonproduction release bucket containing sanitized staging promotion evidence; production GETs only deployments/develop/."
  type        = string
  default     = ""
  validation {
    condition     = var.deployment_tier == "production" ? (var.staging_release_bucket != "" && var.staging_release_bucket != var.release_bucket_name && var.staging_release_bucket != var.state_bucket_name && var.staging_release_bucket != var.notification_bucket_name) : var.staging_release_bucket == ""
    error_message = "Only production may read the distinct staging release bucket."
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
