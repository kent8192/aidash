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
  project_id          = "aidash-fixture"
  byok_project_id     = "aidash-byok-fixture"
  state_bucket_name   = "aidash-fixture-state"
  release_bucket_name = "aidash-fixture-releases"
}
run "deploy_boundary" {
  command   = apply
  state_key = "deploy-boundary"
  variables { byok_broker_environments = ["production", "test"] }
  override_resource {
    target = google_service_account.automation["deploy"]
    values = {
      email = "aidash-deploy@aidash-fixture.iam.gserviceaccount.com"
      name  = "projects/aidash-fixture/serviceAccounts/aidash-deploy@aidash-fixture.iam.gserviceaccount.com"
    }
  }
  override_resource {
    target = google_service_account.broker["production"]
    values = {
      email     = "aidash-production-broker@aidash-fixture.iam.gserviceaccount.com"
      name      = "projects/aidash-fixture/serviceAccounts/aidash-production-broker@aidash-fixture.iam.gserviceaccount.com"
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
    target = google_kms_key_ring.capability["production"]
    values = { id = "projects/aidash-fixture/locations/us-central1/keyRings/aidash-production-capability" }
  }
  override_resource {
    target = google_kms_crypto_key.capability["production"]
    values = { id = "projects/aidash-fixture/locations/us-central1/keyRings/aidash-production-capability/cryptoKeys/capability" }
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
    condition     = toset(google_iam_deny_policy.deploy_broker_boundary[0].rules[0].deny_rule[0].denied_permissions) == toset(["iam.googleapis.com/serviceAccounts.setIamPolicy", "iam.googleapis.com/serviceAccounts.getAccessToken", "iam.googleapis.com/serviceAccounts.signBlob", "iam.googleapis.com/serviceAccounts.signJwt", "iam.googleapis.com/serviceAccounts.implicitDelegation", "iam.googleapis.com/serviceAccountKeys.create"])
    error_message = "The deny covers exactly the impersonation and key-creation permissions of #151."
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
    condition     = keys(google_tags_tag_binding.broker) == ["production", "test"] && google_tags_tag_binding.broker["production"].parent == "//iam.googleapis.com/projects/aidash-fixture/serviceAccounts/100000000000000000001" && google_tags_tag_binding.broker["test"].parent == "//iam.googleapis.com/projects/aidash-fixture/serviceAccounts/100000000000000000002" && alltrue([for b in google_tags_tag_binding.broker : b.tag_value == "tagValues/2001"])
    error_message = "Every broker service account carries the broker tag, and nothing else does."
  }
}
run "no_boundary_without_brokers" {
  command = plan
  assert {
    condition     = length(google_iam_deny_policy.deploy_broker_boundary) == 0 && length(google_tags_tag_key.broker) == 0 && length(google_tags_tag_binding.broker) == 0
    error_message = "A deployment without Credential Broker accounts creates no tag or deny policy."
  }
}
