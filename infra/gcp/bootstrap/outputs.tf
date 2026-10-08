output "configuration" {
  value = {
    project_id                 = var.project_id
    byok_project_id            = var.byok_project_id
    state_bucket               = google_storage_bucket.state.name
    release_bucket             = google_storage_bucket.releases.name
    registry                   = "us-central1-docker.pkg.dev/${var.project_id}/aidash"
    workload_identity_provider = google_iam_workload_identity_pool_provider.github.name
    deploy_service_account     = google_service_account.automation["deploy"].email
    publish_service_account    = google_service_account.automation["publish"].email
  }
}
