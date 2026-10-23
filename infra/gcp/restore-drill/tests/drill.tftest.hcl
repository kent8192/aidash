mock_provider "google" {

  mock_resource "google_service_account" {
    defaults = {
      email  = "drill-nodes@aidash-drill-012345abcdef.iam.gserviceaccount.com"
      member = "serviceAccount:drill-nodes@aidash-drill-012345abcdef.iam.gserviceaccount.com"
    }
  }
  mock_resource "google_project" {
    defaults = { number = "123456789012" }
  }
}
mock_provider "google-beta" {
  mock_resource "google_project_service_identity" {
    defaults = { member = "serviceAccount:service-123456789012@gcp-sa-gkebackup.iam.gserviceaccount.com" }
  }
}
variables {
  project_id      = "aidash-drill-012345abcdef"
  billing_account = "000000-000000-000000"
  folder_id       = "123456789"
  region          = "us-central1"
  backup_plan     = "projects/aidash-production/locations/us/backupPlans/aidash-production-pre-update"
  client_cidr     = "192.0.2.1/32"
  namespace       = "aidash-production"
}
run "isolated_cross_project_restore" {
  command = apply
  assert {
    condition = (
      google_gke_backup_restore_channel.drill.project == "aidash-production" &&
      google_gke_backup_restore_channel.drill.location == "us" &&
      google_gke_backup_restore_channel.drill.destination_project == "projects/123456789012" &&
      google_project_iam_member.source.project == "aidash-production" &&
      google_project_iam_member.source.role == "roles/gkebackup.crossProjectServiceAgent" &&
      google_project_iam_member.source.member == "serviceAccount:service-123456789012@container-engine-robot.iam.gserviceaccount.com"
    )
    error_message = "The source project supplies only this drill's cross-project channel and service-agent permission."
  }
  assert {
    condition = (
      google_gke_backup_restore_plan.drill.project == var.project_id &&
      google_gke_backup_restore_plan.drill.restore_config[0].namespaced_resource_restore_mode == "MERGE_SKIP_ON_CONFLICT" &&
      google_gke_backup_restore_plan.drill.restore_config[0].cluster_resource_restore_scope[0].no_group_kinds &&
      toset(google_gke_backup_restore_plan.drill.restore_config[0].selected_namespaces[0].namespaces) == toset(["aidash-production"]) &&
      google_gke_backup_restore_plan.drill.restore_config[0].volume_data_restore_policy == "RESTORE_VOLUME_DATA_FROM_BACKUP"
    )
    error_message = "The disposable target must support the script's per-Restore exclusions without restoring cluster authority."
  }
  assert {
    condition = (
      google_container_cluster.drill.private_cluster_config[0].enable_private_nodes &&
      google_compute_subnetwork.drill.private_ip_google_access &&
      google_container_cluster.drill.node_config[0].image_type == "UBUNTU_CONTAINERD" &&
      google_project.drill.deletion_policy == "DELETE" &&
      google_gke_backup_restore_channel.drill.deletion_policy == "DELETE"
    )
    error_message = "Use private Standard nodes for the pinned runtime, and release the disposable project/channel on cleanup."
  }
}
