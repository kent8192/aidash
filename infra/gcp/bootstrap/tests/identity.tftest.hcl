mock_provider "google" {}
variables {
  project_id          = "aidash-fixture"
  byok_project_id     = "aidash-byok-fixture"
  state_bucket_name   = "aidash-fixture-state"
  release_bucket_name = "aidash-fixture-releases"
}
run "trusted_workflow_only" {
  command = plan
  assert {
    condition     = strcontains(google_iam_workload_identity_pool_provider.github.attribute_condition, "assertion.repository_id == '1378229915'") && strcontains(google_iam_workload_identity_pool_provider.github.attribute_condition, "gcp-environments.yml@refs/heads/main") && strcontains(google_iam_workload_identity_pool_provider.github.attribute_condition, "assertion.ref == 'refs/heads/main'") && endswith(google_iam_workload_identity_pool_provider.github.attribute_condition, "&& assertion.environment in ['gcp-deploy', 'gcp-deploy-release']")
    error_message = "Name-only or PR-branch OIDC trust is not acceptable."
  }
  assert {
    condition     = google_storage_bucket.state.public_access_prevention == "enforced" && google_storage_bucket.state.force_destroy == false
    error_message = "State must remain private and protected from accidental destruction."
  }
}
run "environment_names_are_not_injectable" {
  command = plan
  variables { deploy_environment = "x' || true || '" }
  expect_failures = [var.deploy_environment]
}
run "release_environment_name_is_not_injectable" {
  command = plan
  variables { release_environment = "x' || true || '" }
  expect_failures = [var.release_environment]
}
run "environments_must_differ" {
  command = plan
  variables { release_environment = "gcp-deploy" }
  expect_failures = [var.release_environment]
}
run "renamed_environments_replace_both_names" {
  command = plan
  variables {
    deploy_environment  = "unattended"
    release_environment = "reviewed"
  }
  assert {
    condition     = endswith(google_iam_workload_identity_pool_provider.github.attribute_condition, "&& assertion.environment in ['unattended', 'reviewed']") && !strcontains(google_iam_workload_identity_pool_provider.github.attribute_condition, "gcp-deploy")
    error_message = "The condition lists exactly the two configured Environments."
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
