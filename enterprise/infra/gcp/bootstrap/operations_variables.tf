# SPDX-FileCopyrightText: 2026 kent8192
# SPDX-License-Identifier: BUSL-1.1
# See enterprise/LICENSE.

variable "production_backup_agent_enabled" {
  description = "Prepare the Backup for GKE API and controller agent flag before the first production cluster deployment; does not create a backup plan."
  type        = bool
  default     = false
  validation {
    condition     = !var.production_backup_agent_enabled || var.deployment_tier == "production"
    error_message = "Only production may prepare its Backup for GKE agent."
  }
}

variable "production_backup_enabled" {
  description = "Opt in only after the existing production cluster and Backup for GKE agent have been observed. Initial bootstrap leaves backups disabled."
  type        = bool
  default     = false
  validation {
    condition     = !var.production_backup_enabled || var.deployment_tier == "production"
    error_message = "Only the isolated production profile may configure production backups."
  }
}
variable "production_monitoring_enabled" {
  description = "Opt in to production health/capacity/restart alerts; does not enable deployment."
  type        = bool
  default     = false
  validation {
    condition     = !var.production_monitoring_enabled || var.deployment_tier == "production"
    error_message = "Only the isolated production profile may configure production monitoring."
  }
}
variable "production_cluster_name" {
  type    = string
  default = "aidash"
  validation {
    condition     = var.production_cluster_name == "aidash"
    error_message = "Operations target the existing Aidash cluster; they do not create another cluster."
  }
}
variable "production_cluster_location" {
  type    = string
  default = "us-central1"
  validation {
    condition     = var.production_cluster_location == "us-central1"
    error_message = "Operations target the existing regional production cluster in us-central1."
  }
}
variable "production_backup_location" {
  description = "Backup artifact region; defaults to the existing cluster region."
  type        = string
  default     = "us-central1"
  validation {
    condition     = contains(["us-central1", "us-east1", "us-west1"], var.production_backup_location)
    error_message = "Use one explicitly supported US backup region. Cross-region backups may incur transfer charges."
  }
}
variable "production_backup_schedule" {
  description = "Daily UTC schedule; 19:00 UTC is 04:00 Asia/Tokyo. Restrict to one daily backup so retention and freshness checks have a clear contract."
  type        = string
  default     = "0 19 * * *"
  validation {
    condition     = can(regex("^([0-9]|[1-5][0-9]) ([0-9]|1[0-9]|2[0-3]) \\* \\* \\*$", var.production_backup_schedule))
    error_message = "Specify one daily UTC backup using 'minute hour * * *'."
  }
}
variable "production_backup_retain_days" {
  type    = number
  default = 30
  validation {
    condition     = var.production_backup_retain_days >= 7 && var.production_backup_retain_days <= 360 && floor(var.production_backup_retain_days) == var.production_backup_retain_days
    error_message = "Daily backups retain between 7 and 360 whole days."
  }
}
variable "production_backup_delete_lock_days" {
  description = "Minimum backup age before deletion. The overall retention policy remains adjustable."
  type        = number
  default     = 7
  validation {
    condition     = var.production_backup_delete_lock_days >= 1 && var.production_backup_delete_lock_days <= 90 && floor(var.production_backup_delete_lock_days) == var.production_backup_delete_lock_days && var.production_backup_delete_lock_days <= var.production_backup_retain_days
    error_message = "Deletion protection is 1..90 whole days and must not exceed retention."
  }
}
variable "production_pre_update_backup_retain_days" {
  type    = number
  default = 14
  validation {
    condition     = var.production_pre_update_backup_retain_days >= 2 && var.production_pre_update_backup_retain_days <= 360 && floor(var.production_pre_update_backup_retain_days) == var.production_pre_update_backup_retain_days
    error_message = "Pre-update backups retain between 2 and 360 whole days."
  }
}
variable "production_pre_update_backup_delete_lock_days" {
  type    = number
  default = 2
  validation {
    condition     = var.production_pre_update_backup_delete_lock_days >= 1 && var.production_pre_update_backup_delete_lock_days <= 90 && floor(var.production_pre_update_backup_delete_lock_days) == var.production_pre_update_backup_delete_lock_days && var.production_pre_update_backup_delete_lock_days <= var.production_pre_update_backup_retain_days
    error_message = "Deletion protection is 1..90 whole days and must not exceed retention."
  }
}
variable "production_backup_schedule_paused" {
  type    = bool
  default = false
}
variable "production_monitoring_hostname" {
  description = "The exact existing production public hostname, never a random preview domain."
  type        = string
  default     = "aidash.run"
  validation {
    condition     = can(regex("^([a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?\\.)+[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$", var.production_monitoring_hostname)) && !endswith(var.production_monitoring_hostname, ".preview.aidash.run")
    error_message = "Monitoring requires an exact production DNS hostname outside the preview namespace."
  }
}
variable "production_volume_utilization_threshold" {
  type    = number
  default = 0.8
  validation {
    condition     = var.production_volume_utilization_threshold >= 0.5 && var.production_volume_utilization_threshold < 1
    error_message = "Capacity warning threshold must be within 0.5..1, exclusive of a completely full disk."
  }
}
variable "production_restart_threshold" {
  description = "Alert when a production container has at least this many restarts in five minutes."
  type        = number
  default     = 5
  validation {
    condition     = var.production_restart_threshold >= 2 && var.production_restart_threshold <= 100 && floor(var.production_restart_threshold) == var.production_restart_threshold
    error_message = "Restart threshold must be an integer in 2..100."
  }
}
variable "delivery_configuration" {
  description = "Nonsecret configuration combined with bootstrap identities for the delivery orchestrator. Null preserves standalone bootstrap compatibility. Deployment remains explicitly disabled outside this configuration."
  type = object({
    cloudflare_zone_id    = string
    domain                = optional(string, "aidash.run")
    develop_branch        = optional(string, "develop/0.1.0")
    trusted_branch        = optional(string, "main")
    release_app_bot_login = string
    preview_tls_secret    = optional(string, "")
    production_origin     = optional(string, "https://aidash.run")
    release_flow_enabled  = optional(bool, false)
  })
  default = null
  validation {
    condition = var.delivery_configuration == null ? true : (
      can(regex("^[a-f0-9]{32}$", var.delivery_configuration.cloudflare_zone_id)) &&
      var.delivery_configuration.domain == "aidash.run" &&
      var.delivery_configuration.trusted_branch == "main" &&
      var.delivery_configuration.develop_branch == var.automation_branch &&
      can(regex("^[a-zA-Z0-9-]+\\[bot\\]$", var.delivery_configuration.release_app_bot_login)) &&
      (var.deployment_tier == "production" ? var.delivery_configuration.preview_tls_secret == "" : can(regex("^[a-zA-Z0-9_-]{1,255}$", var.delivery_configuration.preview_tls_secret))) &&
      var.delivery_configuration.production_origin == "https://${var.production_monitoring_hostname}"
    )
    error_message = "Delivery configuration requires the protected branch/main controller, exact Cloudflare zone, named App bot, tier-matching TLS secret, and the same fixed production origin used by monitoring."
  }
}
