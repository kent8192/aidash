// The current managed Deployment is US. Include Execution Pods and installers;
// the exclusion prevents a second copy in the project's global _Default bucket.
locals {
  data_region_log_filter = "resource.labels.namespace_name=~\"^${local.namespace}(-sandbox|-trusted|-installer)?$\""
}
resource "google_logging_project_bucket_config" "data_region" {
  project        = var.project_id
  location       = "us-central1"
  bucket_id      = "${local.namespace}-regional"
  retention_days = 30
}
resource "google_logging_project_sink" "data_region" {
  project                = var.project_id
  name                   = "${local.namespace}-regional"
  destination            = "logging.googleapis.com/${google_logging_project_bucket_config.data_region.id}"
  filter                 = local.data_region_log_filter
  unique_writer_identity = true
}
resource "google_logging_project_exclusion" "data_region" {
  project     = var.project_id
  name        = "${local.namespace}-regional-only"
  description = "Namespace content is stored only in its US regional bucket."
  filter      = local.data_region_log_filter
  depends_on  = [google_logging_project_sink.data_region]
}
