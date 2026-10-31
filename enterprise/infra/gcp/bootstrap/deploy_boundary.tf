# SPDX-FileCopyrightText: 2026 kent8192
# SPDX-License-Identifier: BUSL-1.1
# See enterprise/LICENSE.

// The deploy pipeline stays part of the trust root for Key Material (ADR 0005), but
// path 1 (impersonating a Credential Broker account) is closed here, by the
// human-run bootstrap that the deploy identity cannot change (#151).
locals {
  broker_boundary = var.byok_project_id != "" && length(var.byok_broker_environments) > 0
  // Deny-policy permission names (service/resource.verb), not the IAM v1 names.
  broker_denied_permissions = [
    "iam.googleapis.com/serviceAccounts.setIamPolicy",
    "iam.googleapis.com/serviceAccounts.getAccessToken",
    "iam.googleapis.com/serviceAccounts.signBlob",
    "iam.googleapis.com/serviceAccounts.signJwt",
    "iam.googleapis.com/serviceAccounts.implicitDelegation",
    "iam.googleapis.com/serviceAccountKeys.create",
    // Bootstrap owns these accounts; deploy never needs to delete, restore, disable
    // them or mint an OpenID token as one.
    "iam.googleapis.com/serviceAccounts.delete",
    "iam.googleapis.com/serviceAccounts.undelete",
    "iam.googleapis.com/serviceAccounts.disable",
    "iam.googleapis.com/serviceAccounts.getOpenIdToken",
  ]
}

// Deny conditions can only select resources by tag, so the broker accounts are
// tagged. The deploy identity holds no tag role and cannot remove the tag.
resource "google_tags_tag_key" "broker" {
  count       = local.broker_boundary ? 1 : 0
  parent      = "projects/${var.project_id}"
  short_name  = "aidash-credential-broker"
  description = "Marks Credential Broker service accounts the deploy identity is denied."
  depends_on  = [google_project_service.required]
}
resource "google_tags_tag_value" "broker" {
  count       = local.broker_boundary ? 1 : 0
  parent      = google_tags_tag_key.broker[0].id
  short_name  = "broker"
  description = "A service account that can read Key Material."
}
resource "google_tags_tag_binding" "broker" {
  for_each  = google_service_account.broker
  parent    = "//iam.googleapis.com/projects/${var.project_id}/serviceAccounts/${each.value.unique_id}"
  tag_value = google_tags_tag_value.broker[0].id
}

resource "google_iam_deny_policy" "deploy_broker_boundary" {
  count        = local.broker_boundary ? 1 : 0
  parent       = urlencode("cloudresourcemanager.googleapis.com/projects/${var.project_id}")
  name         = "aidash-deploy-broker-boundary"
  display_name = "Deploy identity cannot impersonate Credential Broker accounts"
  rules {
    description = "Block path 1: no policy changes, tokens, signatures, keys, deletion or disabling for tagged broker accounts."
    deny_rule {
      denied_principals  = ["principal://iam.googleapis.com/projects/-/serviceAccounts/${google_service_account.automation["deploy"].email}"]
      denied_permissions = local.broker_denied_permissions
      denial_condition {
        title      = "only-credential-broker-accounts"
        expression = "resource.matchTag('${google_tags_tag_key.broker[0].namespaced_name}', '${google_tags_tag_value.broker[0].short_name}')"
      }
    }
  }
  depends_on = [google_tags_tag_binding.broker]
}
