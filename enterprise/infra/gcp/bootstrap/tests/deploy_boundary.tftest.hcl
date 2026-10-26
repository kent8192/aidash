mock_provider "google" {
  mock_resource "google_service_account" {
    defaults = {
      name  = "projects/aidash-fixture/serviceAccounts/automation@aidash-fixture.iam.gserviceaccount.com"
      email = "automation@aidash-fixture.iam.gserviceaccount.com"
    }
  }
  mock_resource "google_tags_tag_key" {
    defaults = { id = "tagKeys/1001", namespaced_name = "aidash-fixture/aidash-credential-broker" }
  }
  mock_resource "google_tags_tag_value" {
    defaults = { id = "tagValues/2001" }
  }
  mock_data "google_project" { defaults = { number = "123456789012" } }
}
variables {
  project_id               = "aidash-fixture"
  byok_project_id          = "aidash-byok-fixture"
  state_bucket_name        = "aidash-fixture-state"
  release_bucket_name      = "aidash-fixture-releases"
  notification_bucket_name = "aidash-fixture-notifications"
  security_alert_emails    = ["security@example.test"]
}
run "deploy_boundary" {
  command   = apply
  state_key = "deploy-boundary"
  variables { byok_broker_environments = ["develop", "test"] }
  override_resource {
    target = google_service_account.automation["deploy"]
    values = {
      email = "aidash-deploy@aidash-fixture.iam.gserviceaccount.com"
      name  = "projects/aidash-fixture/serviceAccounts/aidash-deploy@aidash-fixture.iam.gserviceaccount.com"
    }
  }
  override_resource {
    target = google_service_account.broker["develop"]
    values = {
      email     = "aidash-develop-broker@aidash-fixture.iam.gserviceaccount.com"
      name      = "projects/aidash-fixture/serviceAccounts/aidash-develop-broker@aidash-fixture.iam.gserviceaccount.com"
      unique_id = "100000000000000000001"
    }
  }
  override_resource {
    target = google_service_account.broker["test"]
    values = {
      email     = "aidash-test-broker@aidash-fixture.iam.gserviceaccount.com"
      name      = "projects/aidash-fixture/serviceAccounts/aidash-test-broker@aidash-fixture.iam.gserviceaccount.com"
      unique_id = "100000000000000000002"
    }
  }
  override_resource {
    target = google_kms_key_ring.capability["develop"]
    values = { id = "projects/aidash-fixture/locations/us-central1/keyRings/aidash-develop-capability" }
  }
  override_resource {
    target = google_kms_crypto_key.capability["develop"]
    values = { id = "projects/aidash-fixture/locations/us-central1/keyRings/aidash-develop-capability/cryptoKeys/capability" }
  }
  override_resource {
    target = google_kms_key_ring.capability["test"]
    values = { id = "projects/aidash-fixture/locations/us-central1/keyRings/aidash-test-capability" }
  }
  override_resource {
    target = google_kms_crypto_key.capability["test"]
    values = { id = "projects/aidash-fixture/locations/us-central1/keyRings/aidash-test-capability/cryptoKeys/capability" }
  }
  assert {
    condition     = google_iam_deny_policy.deploy_broker_boundary[0].parent == "cloudresourcemanager.googleapis.com%2Fprojects%2Faidash-fixture"
    error_message = "The deny policy attaches to the shared application project that holds the broker accounts."
  }
  assert {
    condition     = length(google_iam_deny_policy.deploy_broker_boundary[0].rules) == 1 && length(google_iam_deny_policy.deploy_broker_boundary[0].rules[0].deny_rule) == 1
    error_message = "Keep one deny rule so every denied permission shares one tag condition."
  }
  assert {
    condition     = toset(google_iam_deny_policy.deploy_broker_boundary[0].rules[0].deny_rule[0].denied_principals) == toset(["principal://iam.googleapis.com/projects/-/serviceAccounts/aidash-deploy@aidash-fixture.iam.gserviceaccount.com"])
    error_message = "Only the deploy service account is denied."
  }
  assert {
    condition     = toset(google_iam_deny_policy.deploy_broker_boundary[0].rules[0].deny_rule[0].denied_permissions) == toset(["iam.googleapis.com/serviceAccounts.setIamPolicy", "iam.googleapis.com/serviceAccounts.getAccessToken", "iam.googleapis.com/serviceAccounts.signBlob", "iam.googleapis.com/serviceAccounts.signJwt", "iam.googleapis.com/serviceAccounts.implicitDelegation", "iam.googleapis.com/serviceAccountKeys.create", "iam.googleapis.com/serviceAccounts.delete", "iam.googleapis.com/serviceAccounts.undelete", "iam.googleapis.com/serviceAccounts.disable", "iam.googleapis.com/serviceAccounts.getOpenIdToken"])
    error_message = "The deny covers exactly the impersonation, key-creation, deletion and OpenID-token permissions of #151."
  }
  assert {
    condition     = google_iam_deny_policy.deploy_broker_boundary[0].rules[0].deny_rule[0].denial_condition[0].expression == "resource.matchTag('aidash-fixture/aidash-credential-broker', 'broker')" && google_iam_deny_policy.deploy_broker_boundary[0].rules[0].deny_rule[0].exception_principals == null
    error_message = "The deny is scoped by the broker tag and has no principal exception."
  }
  assert {
    condition     = google_tags_tag_key.broker[0].parent == "projects/aidash-fixture" && google_tags_tag_value.broker[0].parent == "tagKeys/1001" && google_tags_tag_value.broker[0].short_name == "broker"
    error_message = "The broker tag lives in the application project."
  }
  assert {
    condition     = keys(google_tags_tag_binding.broker) == ["develop", "test"] && google_tags_tag_binding.broker["develop"].parent == "//iam.googleapis.com/projects/aidash-fixture/serviceAccounts/100000000000000000001" && google_tags_tag_binding.broker["test"].parent == "//iam.googleapis.com/projects/aidash-fixture/serviceAccounts/100000000000000000002" && alltrue([for b in google_tags_tag_binding.broker : b.tag_value == "tagValues/2001"])
    error_message = "Every broker service account carries the broker tag, and nothing else does."
  }
  assert {
    condition     = google_monitoring_alert_policy.byok_key_material_read[0].project == "aidash-byok-fixture" && alltrue([for p in [google_monitoring_alert_policy.broker_revision[0], google_monitoring_alert_policy.broker_iam_policy[0]] : p.project == "aidash-fixture"])
    error_message = "Each alert policy lives in the project that writes its audit log."
  }
  assert {
    condition     = strcontains(google_monitoring_alert_policy.byok_key_material_read[0].conditions[0].condition_matched_log[0].filter, "AccessSecretVersion") && strcontains(google_monitoring_alert_policy.byok_key_material_read[0].conditions[0].condition_matched_log[0].filter, "NOT protoPayload.authenticationInfo.principalEmail=(\"aidash-develop-broker@aidash-fixture.iam.gserviceaccount.com\" OR \"aidash-test-broker@aidash-fixture.iam.gserviceaccount.com\")") && strcontains(google_monitoring_alert_policy.byok_key_material_read[0].conditions[0].condition_matched_log[0].filter, "projects/aidash-byok-fixture/logs/cloudaudit.googleapis.com%2Fdata_access")
    error_message = "Alert on every Key Material read except by the exact broker accounts."
  }
  assert {
    condition     = strcontains(google_monitoring_alert_policy.broker_revision[0].conditions[0].condition_matched_log[0].filter, "(\"aidash-develop-credential-broker\" OR \"aidash-test-credential-broker\")") && strcontains(google_monitoring_alert_policy.broker_revision[0].conditions[0].condition_matched_log[0].filter, "Services.UpdateService")
    error_message = "Alert on every revision of each broker service."
  }
  assert {
    condition     = strcontains(google_monitoring_alert_policy.broker_iam_policy[0].conditions[0].condition_matched_log[0].filter, "Services.SetIamPolicy") && strcontains(google_monitoring_alert_policy.broker_iam_policy[0].conditions[1].condition_matched_log[0].filter, "google.iam.admin.v1.SetIAMPolicy") && strcontains(google_monitoring_alert_policy.broker_iam_policy[0].conditions[1].condition_matched_log[0].filter, "100000000000000000001")
    error_message = "Alert on IAM policy changes of broker services and accounts."
  }
  assert {
    condition     = strcontains(google_monitoring_alert_policy.broker_revision[0].conditions[2].condition_matched_log[0].filter, "compute.googleapis.com") && strcontains(google_monitoring_alert_policy.broker_revision[0].conditions[2].condition_matched_log[0].filter, "instances.setServiceAccount") && strcontains(google_monitoring_alert_policy.broker_revision[0].conditions[3].condition_matched_log[0].filter, "container.googleapis.com") && strcontains(google_monitoring_alert_policy.broker_revision[0].conditions[3].condition_matched_log[0].filter, "CreateNodePool")
    error_message = "A Compute instance or GKE node pool that runs as a broker account must alert."
  }
  assert {
    condition     = google_monitoring_alert_policy.byok_project_iam[0].project == "aidash-byok-fixture" && strcontains(google_monitoring_alert_policy.byok_project_iam[0].conditions[0].condition_matched_log[0].filter, "auditConfigDeltas") && strcontains(google_monitoring_alert_policy.byok_project_iam[0].conditions[1].condition_matched_log[0].filter, "NOT protoPayload.authenticationInfo.principalEmail=\"aidash-deploy@aidash-fixture.iam.gserviceaccount.com\"")
    error_message = "Audit-config changes and non-deploy IAM changes in the BYOK project must alert."
  }
  assert {
    condition     = alltrue([for p in [google_monitoring_alert_policy.broker_revision[0], google_monitoring_alert_policy.broker_iam_policy[0], google_monitoring_alert_policy.byok_key_material_read[0], google_monitoring_alert_policy.byok_project_iam[0]] : length(p.notification_channels) == 1 && p.alert_strategy[0].notification_rate_limit[0].period == "300s"])
    error_message = "Log-match alerts need a rate limit and a recipient."
  }
}
run "alerts_need_a_recipient" {
  command = plan
  variables {
    byok_broker_environments = ["test"]
    security_alert_emails    = []
  }
  expect_failures = [var.security_alert_emails]
}
run "alerts_reject_malformed_recipient" {
  command = plan
  variables { security_alert_emails = ["not-an-email"] }
  expect_failures = [var.security_alert_emails]
}
run "no_boundary_without_brokers" {
  command = plan
  assert {
    condition     = length(google_iam_deny_policy.deploy_broker_boundary) == 0 && length(google_tags_tag_key.broker) == 0 && length(google_tags_tag_binding.broker) == 0
    error_message = "A deployment without Credential Broker accounts creates no tag or deny policy."
  }
}
