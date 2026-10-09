// Human-run bootstrap alone creates identities with BYOK payload access.
// The deploy pipeline is an accepted trust root; deny/WIF hardening is #151.
resource "google_service_account" "broker" {
  for_each     = var.byok_broker_environments
  project      = var.project_id
  account_id   = "aidash-${each.key}-broker"
  display_name = "Aidash ${each.key} Credential Broker"
}
resource "google_project_iam_member" "byok_broker_read" {
  for_each = var.byok_broker_environments
  project  = var.byok_project_id
  role     = google_project_iam_custom_role.byok_broker_read.name
  member   = "serviceAccount:${google_service_account.broker[each.key].email}"
  condition {
    title      = "only-environment-provider-credentials"
    expression = "resource.name.startsWith(\"projects/${data.google_project.byok.number}/secrets/aidash-${each.key}-cred-\")"
  }
}
output "broker_service_accounts" {
  value = { for id, account in google_service_account.broker : id => account.email }
}
