// Broker audit and container logs stay in the existing Deployment's region.
locals {
  data_region_log_filter = "resource.type=\"cloud_run_revision\" AND resource.labels.service_name=\"${local.name}\""
}
resource "google_logging_project_bucket_config" "data_region" {
  count          = local.enabled ? 1 : 0
  project        = var.project_id
  location       = var.region
  bucket_id      = "${local.name}-regional"
  retention_days = 30
}
resource "google_logging_project_sink" "data_region" {
  count                  = local.enabled ? 1 : 0
  project                = var.project_id
  name                   = "${local.name}-regional"
  destination            = "logging.googleapis.com/${google_logging_project_bucket_config.data_region[0].id}"
  filter                 = local.data_region_log_filter
  unique_writer_identity = true
}
resource "google_logging_project_exclusion" "data_region" {
  count       = local.enabled ? 1 : 0
  project     = var.project_id
  name        = "${local.name}-regional-only"
  description = "Broker logs are stored only in the regional log bucket."
  filter      = local.data_region_log_filter
  depends_on  = [google_logging_project_sink.data_region]
}
