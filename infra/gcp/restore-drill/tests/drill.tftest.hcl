mock_provider "google" {

  mock_resource "google_service_account" {
    defaults = {
      name   = "projects/aidash-drill-012345abcdef/serviceAccounts/drill-nodes@aidash-drill-012345abcdef.iam.gserviceaccount.com"
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
override_resource {
  target = google_service_account.workload["server"]
  values = {
    name   = "projects/aidash-drill-012345abcdef/serviceAccounts/drill-server@aidash-drill-012345abcdef.iam.gserviceaccount.com"
    email  = "drill-server@aidash-drill-012345abcdef.iam.gserviceaccount.com"
    member = "serviceAccount:drill-server@aidash-drill-012345abcdef.iam.gserviceaccount.com"
  }
}
override_resource {
  target = google_service_account.workload["worker"]
  values = {
    name   = "projects/aidash-drill-012345abcdef/serviceAccounts/drill-worker@aidash-drill-012345abcdef.iam.gserviceaccount.com"
    email  = "drill-worker@aidash-drill-012345abcdef.iam.gserviceaccount.com"
    member = "serviceAccount:drill-worker@aidash-drill-012345abcdef.iam.gserviceaccount.com"
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
      google_container_cluster.drill.node_config[0].labels["aidash.run/environment"] == "production" &&
      contains(keys(google_project_iam_member.nodes), "roles/container.defaultNodeServiceAccount") &&
      google_container_cluster.drill.workload_identity_config[0].workload_pool == "${var.project_id}.svc.id.goog" &&
      google_container_cluster.drill.node_config[0].workload_metadata_config[0].mode == "GKE_METADATA" &&
      google_container_cluster.drill.addons_config[0].stateful_ha_config[0].enabled &&
      google_service_account_iam_member.workload_identity["server"].member == "serviceAccount:${var.project_id}.svc.id.goog[${var.namespace}/app-aidash-server]" &&
      google_service_account_iam_member.workload_identity["worker"].member == "serviceAccount:${var.project_id}.svc.id.goog[${var.namespace}/app-aidash-worker]" &&
      google_project_iam_member.account_status.project == var.project_id &&
      google_project_iam_custom_role.account_status.permissions == toset(["firebaseauth.users.get"])
    )
    error_message = "The sealed release needs production placement, supported HA resources and isolated workload identity for account-status checks."
  }
  assert {
    condition = (
      google_container_cluster.drill.datapath_provider == "ADVANCED_DATAPATH" &&
      google_container_cluster.drill.private_cluster_config[0].enable_private_nodes &&
      google_compute_subnetwork.drill.private_ip_google_access &&
      google_container_cluster.drill.node_config[0].image_type == "UBUNTU_CONTAINERD" &&
      google_project.drill.deletion_policy == "DELETE" &&
      google_gke_backup_restore_channel.drill.deletion_policy == "DELETE"
    )
    error_message = "Use private Standard nodes for the pinned runtime, and release the disposable project/channel on cleanup."
  }
}
