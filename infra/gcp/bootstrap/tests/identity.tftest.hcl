mock_provider "google" {
  mock_resource "google_service_account" {
    defaults = {
      name  = "projects/aidash-fixture/serviceAccounts/fixture@aidash-fixture.iam.gserviceaccount.com"
      email = "fixture@aidash-fixture.iam.gserviceaccount.com"
    }
  }
}
variables {
  project_id               = "aidash-fixture"
  byok_project_id          = "aidash-byok-fixture"
  state_bucket_name        = "aidash-fixture-state"
  release_bucket_name      = "aidash-fixture-releases"
  notification_bucket_name = "aidash-fixture-notifications"
}
run "trusted_workflow_only" {
  command = plan
  assert {
    condition     = strcontains(google_iam_workload_identity_pool_provider.github.attribute_condition, "assertion.repository_id == '1378229915'") && strcontains(google_iam_workload_identity_pool_provider.github.attribute_condition, "gcp-environments.yml@refs/heads/develop/0.1.0") && strcontains(google_iam_workload_identity_pool_provider.github.attribute_condition, "assertion.ref == 'refs/heads/develop/0.1.0'")
    error_message = "Name-only or PR-branch OIDC trust is not acceptable."
  }
  assert {
    condition     = strcontains(google_iam_workload_identity_pool_provider.github.attribute_condition, "assertion.repository_owner_id == '51869472'") && strcontains(google_iam_workload_identity_pool_provider.github.attribute_condition, "assertion.environment == 'nonproduction'") && strcontains(google_iam_workload_identity_pool_provider.github.attribute_condition, "deployment-notifications.yml@refs/heads/develop/0.1.0") && strcontains(google_iam_workload_identity_pool_provider.github.attribute_condition, "assertion.environment == 'deployment-notifications'")
    error_message = "Deploy and notifier identities require exact protected workflow refs and GitHub environments."
  }
  assert {
    condition     = google_storage_bucket.state.public_access_prevention == "enforced" && google_storage_bucket.state.force_destroy == false
    error_message = "State must remain private and protected from accidental destruction."
  }
}
run "deploy_manages_gke_without_ssh" {
  command = plan
  assert {
    condition     = contains(keys(google_project_service.required), "container.googleapis.com") && !contains(keys(google_project_service.required), "iap.googleapis.com") && !contains(keys(google_project_service.required), "oslogin.googleapis.com")
    error_message = "Environments need the GKE API and no SSH access path."
  }
  assert {
    condition     = contains(keys(google_project_iam_member.deploy), "roles/container.admin") && !contains(keys(google_project_iam_member.deploy), "roles/compute.osAdminLogin") && !contains(keys(google_project_iam_member.deploy), "roles/iap.tunnelResourceAccessor") && !contains(keys(google_project_iam_member.deploy), "roles/compute.securityAdmin")
    error_message = "Deploy administers the cluster and has no OS Login, IAP tunnel or firewall administration."
  }
  assert {
    condition = (
      google_project_iam_member.deploy_node_roles.project == var.project_id &&
      google_project_iam_member.deploy_node_roles.role == "roles/resourcemanager.projectIamAdmin" &&
      google_project_iam_member.deploy_node_roles.condition[0].expression == "api.getAttribute('iam.googleapis.com/modifiedGrantsByRole', []).hasOnly(['roles/container.defaultNodeServiceAccount'])"
    )
    error_message = "Deploy may grant project roles only for the GKE node identity."
  }
}

run "notification_identity_cannot_deploy_or_read_state" {
  command = apply
  override_resource {
    target = google_service_account.automation["notification-reader"]
    values = {
      email = "aidash-notification-reader@aidash-fixture.iam.gserviceaccount.com"
      name  = "projects/aidash-fixture/serviceAccounts/aidash-notification-reader@aidash-fixture.iam.gserviceaccount.com"
    }
  }
  override_resource {
    target = google_service_account.automation["deploy"]
    values = {
      email = "aidash-deploy@aidash-fixture.iam.gserviceaccount.com"
      name  = "projects/aidash-fixture/serviceAccounts/aidash-deploy@aidash-fixture.iam.gserviceaccount.com"
    }
  }
  override_resource {
    target = google_iam_workload_identity_pool.github
    values = { name = "projects/123456789012/locations/global/workloadIdentityPools/aidash-github" }
  }
  assert {
    condition = (
      endswith(google_service_account_iam_member.github["notification-reader"].member, "/attribute.workflow_role/notification-reader") &&
      endswith(google_service_account_iam_member.github["deploy"].member, "/attribute.workflow_role/deploy") &&
      google_service_account_iam_member.github["deploy"].member != google_service_account_iam_member.github["notification-reader"].member &&
      alltrue([for grant in google_project_iam_member.deploy : grant.member != "serviceAccount:aidash-notification-reader@aidash-fixture.iam.gserviceaccount.com"])
    )
    error_message = "Notification assertions must not impersonate deploy/publish or receive project admin roles."
  }
  assert {
    condition = (
      google_storage_bucket.notifications.name != google_storage_bucket.state.name &&
      google_storage_bucket.notifications.name != google_storage_bucket.releases.name &&
      google_storage_bucket.notifications.public_access_prevention == "enforced" &&
      google_storage_bucket_iam_member.notification_reader.bucket == google_storage_bucket.notifications.name &&
      google_storage_bucket_iam_member.notification_reader.role == "roles/storage.objectViewer" &&
      google_storage_bucket_iam_member.notification_delivery.condition[0].expression == "resource.name.startsWith('projects/_/buckets/aidash-fixture-notifications/objects/notification-delivery/')" &&
      google_storage_bucket_iam_member.notification_receipt_writer.condition[0].expression == "resource.name.startsWith('projects/_/buckets/aidash-fixture-notifications/objects/notifications/')" &&
      google_storage_bucket_iam_member.state.member == "serviceAccount:aidash-deploy@aidash-fixture.iam.gserviceaccount.com"
    )
    error_message = "Notification data must live outside secret-bearing state with claims confined to their prefix."
  }
}
