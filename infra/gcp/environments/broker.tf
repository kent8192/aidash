variable "credential_brokers" {
  description = "Explicit opt-in only for environments with a Provider Credential Store. Never populate for PR previews."
  type = map(object({
    enabled                      = optional(bool, false)
    byok_project_id              = string
    secret_prefix                = string
    broker_service_account_email = string
    signing_key_id               = string
    signing_version              = optional(string, "1")
    verification_versions        = optional(set(string), ["1"])
    image                        = string
  }))
  default = {}
  validation {
    condition     = alltrue([for id, broker in var.credential_brokers : !broker.enabled || try(var.environments[id].kind != "pr", false)])
    error_message = "Enabled brokers require an existing non-PR environment."
  }
  validation {
    condition     = alltrue([for id, broker in var.credential_brokers : !broker.enabled || (var.byok_project_id != "" && broker.byok_project_id == var.byok_project_id)])
    error_message = "Enabled brokers must use the environment's configured Provider Credential Store project."
  }
  validation {
    condition     = alltrue([for id, broker in var.credential_brokers : !broker.enabled || alltrue([for other in keys(var.environments) : id == other || (!startswith("aidash-${other}-cred-", "aidash-${id}-cred-") && !startswith("aidash-${id}-cred-", "aidash-${other}-cred-"))])])
    error_message = "Broker credential prefixes must not overlap any managed environment's credential namespace."
  }
}
module "credential_broker" {
  source   = "../modules/credential-broker"
  for_each = { for id, broker in var.credential_brokers : id => broker if broker.enabled && try(var.environments[id].kind != "pr", false) }

  enabled                      = true
  bind_runtime_signer          = false
  project_id                   = var.project_id
  byok_project_id              = each.value.byok_project_id
  environment_id               = each.key
  environment_kind             = var.environments[each.key].kind
  secret_prefix                = each.value.secret_prefix
  image                        = each.value.image
  deploy_service_account       = var.deploy_service_account
  broker_service_account_email = each.value.broker_service_account_email
  signing_key_id               = each.value.signing_key_id
  signing_version              = each.value.signing_version
  verification_versions        = each.value.verification_versions
}
// Wire IAM after both modules exist: broker -> VM settings must not create a
// reverse module dependency through the worker's service-account output.
resource "google_kms_crypto_key_iam_member" "broker_signer" {
  for_each      = module.credential_broker
  crypto_key_id = each.value.signing_key
  role          = "roles/cloudkms.signer"
  member        = "serviceAccount:${module.environment[each.key].runtime_service_account}"
}
// Preserve existing bindings when upgrading previously enabled managed brokers.
moved {
  from = module.credential_broker["test"].google_kms_crypto_key_iam_member.signer[0]
  to   = google_kms_crypto_key_iam_member.broker_signer["test"]
}
moved {
  from = module.credential_broker["develop"].google_kms_crypto_key_iam_member.signer[0]
  to   = google_kms_crypto_key_iam_member.broker_signer["develop"]
}
output "credential_brokers" { value = { for id, broker in module.credential_broker : id => broker.worker_configuration } }
output "managed_credential_brokers" {
  description = "Non-secret applied broker intent, including disabled entries, for controller reconciliation."
  value       = { for id, broker in var.credential_brokers : id => broker if contains(keys(var.environments), id) }
}
