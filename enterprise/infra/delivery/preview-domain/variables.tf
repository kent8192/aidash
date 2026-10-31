# SPDX-FileCopyrightText: 2026 kent8192
# SPDX-License-Identifier: BUSL-1.1
# See enterprise/LICENSE.

variable "account_id" {
  type = string
  validation {
    condition     = can(regex("^[a-f0-9]{32}$", var.account_id))
    error_message = "Use the exact Cloudflare account ID; do not infer it from a zone ID."
  }
}

variable "zone_id" {
  type = string
  validation {
    condition     = can(regex("^[a-f0-9]{32}$", var.zone_id))
    error_message = "Use the exact 32-character Cloudflare zone ID."
  }
}

variable "domain" {
  type    = string
  default = "aidashusercontent.app"
  validation {
    condition     = var.domain == "aidashusercontent.app"
    error_message = "This module is scoped to the Cloud preview domain aidashusercontent.app."
  }
}

variable "psl_pull_request_url" {
  description = "URL of the publicsuffix/list pull request that adds aidashusercontent.app; null until one is opened."
  type        = string
  default     = null
  validation {
    condition     = var.psl_pull_request_url == null || can(regex("^https://github\\.com/publicsuffix/list/pull/[1-9][0-9]*$", var.psl_pull_request_url))
    error_message = "Use the exact https://github.com/publicsuffix/list/pull/<number> URL."
  }
}

variable "cloudflare_api_token" {
  description = "Private bootstrap token; ephemeral so it is never saved in Terraform plans or state."
  type        = string
  default     = null
  sensitive   = true
  ephemeral   = true
}
