# SPDX-FileCopyrightText: 2026 kent8192
# SPDX-License-Identifier: BUSL-1.1
# See enterprise/LICENSE.

variable "nonproduction_project_id" {
  type = string
  validation {
    condition     = can(regex("^[a-z][a-z0-9-]{4,28}[a-z0-9]$", var.nonproduction_project_id))
    error_message = "Use an explicit existing nonproduction project ID."
  }
}

variable "production_project_id" {
  type = string
  validation {
    condition     = can(regex("^[a-z][a-z0-9-]{4,28}[a-z0-9]$", var.production_project_id)) && var.production_project_id != var.nonproduction_project_id
    error_message = "Use an explicit production project ID distinct from nonproduction."
  }
}

variable "billing_account_id" {
  type = string
  validation {
    condition     = can(regex("^[A-F0-9]{6}-[A-F0-9]{6}-[A-F0-9]{6}$", var.billing_account_id))
    error_message = "Use the explicitly selected billing account ID."
  }
}

variable "organization_id" {
  type    = string
  default = null
  validation {
    condition     = var.organization_id == null || can(regex("^[0-9]+$", var.organization_id))
    error_message = "Organization ID must be numeric or unset."
  }
}

variable "folder_id" {
  type    = string
  default = null
  validation {
    condition     = var.folder_id == null || can(regex("^[0-9]+$", var.folder_id))
    error_message = "Folder ID must be numeric or unset."
  }
  validation {
    condition     = var.folder_id == null || var.organization_id == null
    error_message = "Select one parent organization or folder, preserving the approved ownership boundary."
  }
}

variable "existing_production_project" {
  description = "Opt in only after verifying ownership of an already-existing exact project ID."
  type        = bool
  default     = false
}
