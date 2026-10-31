# SPDX-FileCopyrightText: 2026 kent8192
# SPDX-License-Identifier: BUSL-1.1
# See enterprise/LICENSE.

output "configuration" {
  value = {
    project_id                      = var.project_id
    deployment_tier                 = var.deployment_tier
    automation_branch               = var.automation_branch
    nonproduction_project_id        = var.nonproduction_project_id
    nonproduction_state_bucket      = var.nonproduction_state_bucket
    production_node_service_account = try(google_service_account.production_nodes[0].email, null)
    staging_project_id              = var.deployment_tier == "production" ? var.nonproduction_project_id : ""
    staging_release_bucket          = var.staging_release_bucket
    notification_bucket             = google_storage_bucket.notifications.name
    notification_service_account    = google_service_account.automation["notification-reader"].email
    byok_project_id                 = var.byok_project_id
    broker_service_accounts         = { for id, account in google_service_account.broker : id => account.email }
    state_bucket                    = google_storage_bucket.state.name
    release_bucket                  = google_storage_bucket.releases.name
    registry                        = "us-central1-docker.pkg.dev/${var.project_id}/aidash"
    workload_identity_provider      = google_iam_workload_identity_pool_provider.github.name
    deploy_service_account          = google_service_account.automation["deploy"].email
    publish_service_account         = google_service_account.automation["publish"].email
  }
}
