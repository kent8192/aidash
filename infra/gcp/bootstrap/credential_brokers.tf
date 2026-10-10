// Human-run bootstrap alone creates identities with BYOK payload access.
// The deploy pipeline is an accepted trust root; deny/WIF hardening is #151.
resource "google_service_account" "broker" {
  for_each     = var.byok_project_id != "" ? var.byok_broker_environments : toset([])
  project      = var.project_id
  account_id   = "aidash-${each.key}-broker"
  display_name = "Aidash ${each.key} Credential Broker"
  depends_on   = [google_project_service.required]
}
// Immutable signing-key names outlive automation's broker enablement and VM
// retirement. Bootstrap owns their state; operators must retain this namespace.
resource "google_kms_key_ring" "capability" {
  for_each = var.byok_project_id != "" ? var.byok_broker_environments : toset([])
  project  = var.project_id
  location = var.broker_signing_region
  name     = "aidash-${each.key}-capability"
  lifecycle { prevent_destroy = true }
  depends_on = [google_project_service.required]
}
resource "google_kms_crypto_key" "capability" {
  for_each = google_kms_key_ring.capability
  key_ring = each.value.id
  name     = "capability"
  purpose  = "ASYMMETRIC_SIGN"
  version_template {
    algorithm        = "EC_SIGN_ED25519"
    protection_level = "SOFTWARE"
  }
  lifecycle { prevent_destroy = true }
}
resource "google_project_iam_member" "byok_broker_read" {
  for_each = var.byok_project_id != "" ? var.byok_broker_environments : toset([])
  project  = var.byok_project_id
  role     = google_project_iam_custom_role.byok_broker_read[0].name
  member   = "serviceAccount:${google_service_account.broker[each.key].email}"
  condition {
    title      = "only-environment-provider-credentials"
    expression = "resource.name.startsWith(\"projects/${data.google_project.byok[0].number}/secrets/aidash-${each.key}-cred-\")"
  }
}
// Deploy plans read verification PEMs without receiving signing authority.
resource "google_kms_crypto_key_iam_member" "deploy_capability_public_key" {
  for_each      = google_kms_crypto_key.capability
  crypto_key_id = each.value.id
  role          = "roles/cloudkms.publicKeyViewer"
  member        = "serviceAccount:${google_service_account.automation["deploy"].email}"
}
output "broker_service_accounts" {
  value = { for id, account in google_service_account.broker : id => account.email }
}
output "broker_signing_keys" {
  value = { for id, key in google_kms_crypto_key.capability : id => key.id }
}
