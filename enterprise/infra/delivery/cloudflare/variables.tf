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
  default = "aidash.run"
  validation {
    condition     = var.domain == "aidash.run"
    error_message = "This module is scoped to the existing aidash.run zone."
  }
}

variable "destination_address_id" {
  description = "Existing, verified Email Routing destination ID in this account."
  type        = string
  validation {
    condition     = can(regex("^[a-f0-9]{32}$", var.destination_address_id))
    error_message = "Supply the existing verified destination address ID."
  }
}

variable "sending_subdomain_id" {
  description = "Existing configured aidash.run Email Sending registration ID, discovered without onboarding."
  type        = string
  validation {
    condition     = can(regex("^[a-f0-9]{32}$", var.sending_subdomain_id))
    error_message = "Supply the existing configured Email Sending registration ID."
  }
}

variable "destination_email" {
  description = "Expected private verified destination; treat state and plans as private."
  type        = string
  sensitive   = true
  validation {
    condition     = can(regex("^[A-Za-z0-9.!#$%&'*+/=?^_`{|}~-]+@[A-Za-z0-9](?:[A-Za-z0-9.-]*[A-Za-z0-9])?$", var.destination_email)) && length(var.destination_email) <= 254
    error_message = "Supply one mailbox, without display names, commas, or header line breaks."
  }
}
variable "cloudflare_api_token" {
  description = "Private bootstrap token; ephemeral so it is never saved in Terraform plans or state."
  type        = string
  default     = null
  sensitive   = true
  ephemeral   = true
}

variable "configure_routing_dns" {
  description = "Opt in to guarded Cloudflare-managed Routing DNS setup; imports existing enablement rather than onboarding Sending."
  type        = bool
  default     = false
}
