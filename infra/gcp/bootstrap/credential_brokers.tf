// Human-run bootstrap alone creates identities with Secret Store payload access.
// The deploy pipeline is an accepted trust root; deploy_boundary.tf (#151) denies
// it every way of impersonating these accounts.
locals {
  broker_environments = var.byok_project_id != "" ? var.byok_broker_environments : toset([])
}
resource "google_service_account" "broker" {
  for_each     = local.broker_environments
  project      = var.project_id
  account_id   = "aidash-${each.key}-broker"
  display_name = "Aidash ${each.key} Credential Broker"
  depends_on   = [google_project_service.required]
}
// A second identity reads Tenant Secrets so a fault in the forwarder that sends
// to Tenant-chosen hosts cannot reach Key Material (ADR 0095). GCP limits an
// account ID to 30 characters, so the suffix is "-tsec" rather than the
// suggested "-tsec-broker": environment IDs may reach 16 characters.
resource "google_service_account" "tenant_secret_broker" {
  for_each     = local.broker_environments
  project      = var.project_id
  account_id   = "aidash-${each.key}-tsec"
  display_name = "Aidash ${each.key} Tenant Secret Broker"
  depends_on   = [google_project_service.required]
}
// Cloud Run needs actAs to run either broker service as these accounts. The
// grants live here because deploy_boundary.tf denies the deploy identity
// setIamPolicy on them; actAs itself stays allowed (broker redeploy is the
// accepted residual path 2).
resource "google_service_account_iam_member" "deploy_act_as_broker" {
  for_each           = google_service_account.broker
  service_account_id = each.value.name
  role               = "roles/iam.serviceAccountUser"
  member             = "serviceAccount:${google_service_account.automation["deploy"].email}"
}
resource "google_service_account_iam_member" "deploy_act_as_tenant_secret_broker" {
  for_each           = google_service_account.tenant_secret_broker
  service_account_id = each.value.name
  role               = "roles/iam.serviceAccountUser"
  member             = "serviceAccount:${google_service_account.automation["deploy"].email}"
}
// Immutable signing-key names outlive automation's broker enablement and VM
// retirement. Bootstrap owns their state; whoever runs the Deployment must retain this namespace.
resource "google_kms_key_ring" "capability" {
  for_each = local.broker_environments
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
// The server signs Tenant Secret Capability Tokens with this key, never with the
// worker's: the broker can then tell the issuers apart and revoke one alone
// (ADR 0005 amendment, ADR 0095).
resource "google_kms_crypto_key" "tenant_secret" {
  for_each = google_kms_key_ring.capability
  key_ring = each.value.id
  name     = "tenant-secret"
  purpose  = "ASYMMETRIC_SIGN"
  version_template {
    algorithm        = "EC_SIGN_ED25519"
    protection_level = "SOFTWARE"
  }
  lifecycle { prevent_destroy = true }
}
// The Key Material broker sees only -cred- secrets; the Tenant Secret broker sees
// -tsec- and -oauth- secrets. Neither can read the other's payloads.
resource "google_project_iam_member" "byok_broker_read" {
  for_each = local.broker_environments
  project  = var.byok_project_id
  role     = google_project_iam_custom_role.byok_broker_read[0].name
  member   = "serviceAccount:${google_service_account.broker[each.key].email}"
  condition {
    title      = "only-environment-provider-credentials"
    expression = "resource.name.startsWith(\"projects/${data.google_project.byok[0].number}/secrets/aidash-${each.key}-cred-\")"
  }
}
resource "google_project_iam_member" "byok_tenant_secret_broker_read" {
  for_each = local.broker_environments
  project  = var.byok_project_id
  role     = google_project_iam_custom_role.byok_tenant_secret_broker_read[0].name
  member   = "serviceAccount:${google_service_account.tenant_secret_broker[each.key].email}"
  condition {
    title      = "only-environment-tenant-secrets"
    expression = "resource.name.startsWith(\"projects/${data.google_project.byok[0].number}/secrets/aidash-${each.key}-tsec-\")"
  }
}
// The existing Tenant Secret identity alone exchanges and rotates OAuth grants.
// Its add-version right is confined to -oauth-, never -tsec- or -cred-.
resource "google_project_iam_member" "byok_oauth_broker" {
  for_each = local.broker_environments
  project  = var.byok_project_id
  role     = google_project_iam_custom_role.byok_oauth_broker[0].name
  member   = "serviceAccount:${google_service_account.tenant_secret_broker[each.key].email}"
  condition {
    title      = "only-environment-oauth-connections"
    expression = "resource.name.startsWith(\"projects/${data.google_project.byok[0].number}/secrets/aidash-${each.key}-oauth-\")"
  }
}
// Deploy plans read verification PEMs without receiving signing authority.
resource "google_kms_crypto_key_iam_member" "deploy_capability_public_key" {
  for_each      = google_kms_crypto_key.capability
  crypto_key_id = each.value.id
  role          = "roles/cloudkms.publicKeyViewer"
  member        = "serviceAccount:${google_service_account.automation["deploy"].email}"
}
resource "google_kms_crypto_key_iam_member" "deploy_tenant_secret_public_key" {
  for_each      = google_kms_crypto_key.tenant_secret
  crypto_key_id = each.value.id
  role          = "roles/cloudkms.publicKeyViewer"
  member        = "serviceAccount:${google_service_account.automation["deploy"].email}"
}
output "broker_service_accounts" {
  value = { for id, account in google_service_account.broker : id => account.email }
}
output "tenant_secret_broker_service_accounts" {
  value = { for id, account in google_service_account.tenant_secret_broker : id => account.email }
}
output "broker_signing_keys" {
  value = { for id, key in google_kms_crypto_key.capability : id => key.id }
}
output "tenant_secret_signing_keys" {
  value = { for id, key in google_kms_crypto_key.tenant_secret : id => key.id }
}
