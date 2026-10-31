// The Credential Broker image digest is reviewed code, not deployment configuration
// (#151): changing the broker means changing broker-image.json in a pull request.
// Update it with the procedure in infra/gcp/README.md; null means no image has been
// approved, so no broker can be enabled. The credential_brokers type has no image
// attribute on purpose: a stale configuration that still sets one fails type
// conversion, and the controller's intent comparison sees identical config and state.
variable "broker_image_lock_file" {
  description = "Test seam for the digest lock. The controller never sets it; deployments read broker-image.json from the reviewed checkout."
  type        = string
  default     = null
}
locals {
  broker_image_digest = try(jsondecode(file(coalesce(var.broker_image_lock_file, "${path.module}/broker-image.json"))).digest, null)
  // Artifact Registry is fixed to us-central1 by bootstrap; deploy publishes images there.
  broker_image = local.broker_image_digest == null ? null : "us-central1-docker.pkg.dev/${var.project_id}/aidash/broker@${local.broker_image_digest}"
  // The server signs Tenant Secret Capability Tokens with a key of its own, and
  // the broker accepts both issuers' public keys (ADR 0005 amendment, ADR 0095).
  // The worker's existing issuer must stay identical for provider and Tenant
  // Secret tokens, so it is named once here and passed to both brokers.
  tenant_secret_issuer = "aidash-server"
  worker_issuer        = "aidash-worker"
}

variable "credential_brokers" {
  description = "Explicit opt-in only for environments with a Secret Store. Never populate for PR previews."
  type = map(object({
    enabled                                    = optional(bool, false)
    byok_project_id                            = string
    secret_prefix                              = string
    broker_service_account_email               = string
    signing_key_id                             = string
    signing_version                            = optional(string, "1")
    verification_versions                      = optional(set(string), ["1"])
    tenant_secret_broker_service_account_email = optional(string, "")
    tenant_secret_signing_key_id               = optional(string, "")
    tenant_secret_signing_version              = optional(string, "1")
    tenant_secret_verification_versions        = optional(set(string), ["1"])
  }))
  default = {}
  validation {
    condition     = alltrue([for id, broker in var.credential_brokers : !broker.enabled || try(var.environments[id].kind != "pr", false)])
    error_message = "Enabled brokers require an existing non-PR environment."
  }
  validation {
    condition     = alltrue([for id, broker in var.credential_brokers : !broker.enabled || can(regex("^sha256:[a-f0-9]{64}$", local.broker_image_digest))])
    error_message = "Enabled brokers need an approved image digest in infra/gcp/environments/broker-image.json."
  }
  validation {
    condition     = alltrue([for id, broker in var.credential_brokers : !broker.enabled || (var.byok_project_id != "" && broker.byok_project_id == var.byok_project_id)])
    error_message = "Enabled brokers must use the environment's configured Secret Store project."
  }
  validation {
    condition     = alltrue([for id, broker in var.credential_brokers : !broker.enabled || alltrue([for other in keys(var.environments) : id == other || alltrue(flatten([for a in ["cred", "tsec", "oauth"] : [for b in ["cred", "tsec", "oauth"] : !startswith("aidash-${other}-${b}-", "aidash-${id}-${a}-") && !startswith("aidash-${id}-${a}-", "aidash-${other}-${b}-")]]))])])
    error_message = "Broker credential prefixes must not overlap any managed environment's credential namespace."
  }
  validation {
    condition     = alltrue([for id, broker in var.credential_brokers : !broker.enabled || (broker.tenant_secret_broker_service_account_email != "" && broker.tenant_secret_signing_key_id != "")])
    error_message = "Enabled brokers also need the bootstrap Tenant Secret broker service account and server signing key."
  }
}
module "credential_broker" {
  source   = "../modules/credential-broker"
  for_each = { for id, broker in var.credential_brokers : id => broker if broker.enabled && try(var.environments[id].kind != "pr", false) }

  enabled                      = true
  bind_runtime_signer          = false
  role                         = "key-material"
  project_id                   = var.project_id
  deployment_tier              = var.deployment_tier
  byok_project_id              = each.value.byok_project_id
  environment_id               = each.key
  environment_kind             = var.environments[each.key].kind
  secret_prefix                = each.value.secret_prefix
  image                        = local.broker_image
  broker_service_account_email = each.value.broker_service_account_email
  signing_key_id               = each.value.signing_key_id
  signing_version              = each.value.signing_version
  verification_versions        = each.value.verification_versions
  issuer                       = local.worker_issuer
}
// This same service serves /v1/connections/{exchange,refresh,revoke} and
// /v1/forward connection claims. It derives -oauth- from its -tsec- prefix.
module "tenant_secret_broker" {
  source   = "../modules/credential-broker"
  for_each = { for id, broker in var.credential_brokers : id => broker if broker.enabled && try(var.environments[id].kind != "pr", false) }

  enabled                      = true
  bind_runtime_signer          = false
  role                         = "tenant-secrets"
  project_id                   = var.project_id
  deployment_tier              = var.deployment_tier
  byok_project_id              = each.value.byok_project_id
  environment_id               = each.key
  environment_kind             = var.environments[each.key].kind
  secret_prefix                = "aidash-${each.key}-tsec-"
  image                        = local.broker_image
  broker_service_account_email = each.value.tenant_secret_broker_service_account_email
  signing_key_id               = each.value.tenant_secret_signing_key_id
  signing_version              = each.value.tenant_secret_signing_version
  verification_versions        = each.value.tenant_secret_verification_versions
  issuer                       = local.tenant_secret_issuer
  // The Tenant Secret broker accepts both issuers' public keys and maps each
  // verification key to the issuer its tokens must carry (ADR 0005 amendment).
  secondary_signing_key_id        = each.value.signing_key_id
  secondary_verification_versions = each.value.verification_versions
  secondary_issuer                = local.worker_issuer
}
// Wire IAM after both modules exist: broker -> worker/server settings must not
// create a reverse module dependency through the runtime service-account output.
resource "google_kms_crypto_key_iam_member" "broker_signer" {
  for_each      = module.credential_broker
  crypto_key_id = each.value.signing_key
  role          = "roles/cloudkms.signer"
  member        = "serviceAccount:${module.environment[each.key].worker_service_account}"
}
resource "google_kms_crypto_key_iam_member" "tenant_secret_signer" {
  for_each      = module.tenant_secret_broker
  crypto_key_id = each.value.signing_key
  role          = "roles/cloudkms.signer"
  member        = "serviceAccount:${module.environment[each.key].server_service_account}"
}
output "credential_brokers" { value = { for id, broker in module.credential_broker : id => broker.worker_configuration } }
output "tenant_secret_brokers" { value = { for id, broker in module.tenant_secret_broker : id => broker.server_configuration } }
output "managed_credential_brokers" {
  description = "Non-secret applied broker intent, including disabled entries, for controller reconciliation."
  value       = { for id, broker in var.credential_brokers : id => broker if contains(keys(var.environments), id) }
}
