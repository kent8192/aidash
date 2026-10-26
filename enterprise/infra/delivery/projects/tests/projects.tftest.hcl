# SPDX-FileCopyrightText: 2026 kent8192
# SPDX-License-Identifier: BUSL-1.1
# See enterprise/LICENSE.

mock_provider "google" {
  mock_data "google_project" {
    defaults = { project_id = "aidash-nonproduction" }
  }
}

variables {
  nonproduction_project_id = "aidash-nonproduction"
  production_project_id    = "aidash-production"
  billing_account_id       = "000000-000000-000000"
}

run "isolated_production_without_default_network" {
  command = plan
  assert {
    condition     = google_project.production.project_id != data.google_project.nonproduction.project_id && google_project.production.deletion_policy == "PREVENT" && !google_project.production.auto_create_network
    error_message = "Production project must be isolated, retained and without an unintended default network."
  }
}

run "shared_project_refused" {
  command = plan
  variables { production_project_id = "aidash-nonproduction" }
  expect_failures = [var.production_project_id]
}

run "ambiguous_parent_refused" {
  command = plan
  variables {
    organization_id = "123456789"
    folder_id       = "987654321"
  }
  expect_failures = [var.folder_id]
}
