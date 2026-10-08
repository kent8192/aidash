mock_provider "google" {
  mock_data "google_project" {
    defaults = { number = "123456789012" }
  }
}

variables {
  project_id          = "aidash-fixture"
  byok_project_id     = "aidash-byok-fixture"
  state_bucket_name   = "aidash-fixture-state"
  release_bucket_name = "aidash-fixture-releases"
}

run "fixed_roles_and_limited_deploy" {
  command = plan
  override_resource {
    override_during = plan
    target          = google_project_iam_custom_role.byok_deploy
    values          = { name = "projects/aidash-byok-fixture/roles/aidashByokDeployment" }
  }
  override_resource {
    override_during = plan
    target          = google_service_account.automation["deploy"]
    values = {
      email = "aidash-deploy@aidash-fixture.iam.gserviceaccount.com"
      name  = "projects/aidash-fixture/serviceAccounts/aidash-deploy@aidash-fixture.iam.gserviceaccount.com"
    }
  }
  assert {
    condition = (
      google_project_iam_custom_role.byok_create.role_id == "aidashByokCreate" &&
      google_project_iam_custom_role.byok_create.permissions == toset(["secretmanager.secrets.create"]) &&
      google_project_iam_custom_role.byok_manage.role_id == "aidashByokManage" &&
      google_project_iam_custom_role.byok_manage.permissions == toset([
        "secretmanager.secrets.get", "secretmanager.secrets.delete", "secretmanager.versions.add",
        "secretmanager.versions.disable", "secretmanager.versions.destroy",
        "secretmanager.versions.get", "secretmanager.versions.list",
      ]) &&
      google_project_iam_custom_role.byok_broker_read.role_id == "aidashByokBrokerRead" &&
      google_project_iam_custom_role.byok_broker_read.permissions == toset([
        "secretmanager.versions.access", "secretmanager.versions.get", "secretmanager.secrets.get",
      ])
    )
    error_message = "Bootstrap must own the three fixed, least-privilege BYOK roles."
  }
  assert {
    condition = (
      google_project_iam_custom_role.byok_deploy.permissions == toset([
        "resourcemanager.projects.get", "resourcemanager.projects.getIamPolicy",
        "resourcemanager.projects.setIamPolicy",
      ]) &&
      google_project_iam_member.byok_deploy.project == var.byok_project_id &&
      google_project_iam_member.byok_deploy.role == "projects/aidash-byok-fixture/roles/aidashByokDeployment" &&
      google_project_iam_member.byok_deploy.member == "serviceAccount:aidash-deploy@aidash-fixture.iam.gserviceaccount.com" &&
      length(google_project_iam_member.byok_deploy.condition) == 1 &&
      google_project_iam_member.byok_deploy.condition[0].expression == "api.getAttribute('iam.googleapis.com/modifiedGrantsByRole', []).hasOnly(['projects/aidash-byok-fixture/roles/aidashByokCreate', 'projects/aidash-byok-fixture/roles/aidashByokManage'])"
    )
    error_message = "Deploy must have no role-edit permission and only the exact Create/Manage role-grant allowlist."
  }
}
