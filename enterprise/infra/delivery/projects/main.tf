# SPDX-FileCopyrightText: 2026 kent8192
# SPDX-License-Identifier: BUSL-1.1
# See enterprise/LICENSE.

data "google_project" "nonproduction" {
  project_id = var.nonproduction_project_id
}

// Billing checks use the explicitly selected nonproduction quota project.
// API enablement is part of this human-run creation stack, not a dashboard step.
resource "google_project_service" "cloud_billing" {
  project            = var.nonproduction_project_id
  service            = "cloudbilling.googleapis.com"
  disable_on_destroy = false
  lifecycle { prevent_destroy = true }
}

// Project creation is a separate bootstrap phase; it neither creates a cluster
// nor enables production delivery. Existing projects must be explicitly adopted.
resource "google_project" "production" {
  project_id          = var.production_project_id
  name                = "Aidash production"
  billing_account     = var.billing_account_id
  org_id              = var.organization_id
  folder_id           = var.folder_id
  auto_create_network = false
  deletion_policy     = "PREVENT"
  labels = {
    application = "aidash"
    environment = "production"
  }
  depends_on = [google_project_service.cloud_billing]
  lifecycle {
    prevent_destroy = true
    precondition {
      condition     = var.production_project_id != data.google_project.nonproduction.project_id
      error_message = "Production must use a different project from nonproduction."
    }
  }
}

import {
  for_each = var.existing_production_project ? { production = var.production_project_id } : {}
  to       = google_project.production
  id       = each.value
}

output "production_project_id" {
  value = google_project.production.project_id
}

output "nonproduction_project_id" {
  value = data.google_project.nonproduction.project_id
}
