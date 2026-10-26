# SPDX-FileCopyrightText: 2026 kent8192
# SPDX-License-Identifier: BUSL-1.1
# See enterprise/LICENSE.

// The Credential Broker image digest is reviewed code, not deployment configuration
// (#151): changing the broker means changing broker-image.json in a pull request.
// Update it with the procedure in enterprise/infra/gcp/README.md; null means no image has been
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
}

variable "credential_brokers" {
  description = "Explicit opt-in only for environments with a Secret Store. Never populate for PR previews."
  type = map(object({
    enabled                      = optional(bool, false)
    byok_project_id              = string
    secret_prefix                = string
    broker_service_account_email = string
    signing_key_id               = string
    signing_version              = optional(string, "1")
    verification_versions        = optional(set(string), ["1"])
  }))
  default = {}
  validation {
    condition     = alltrue([for id, broker in var.credential_brokers : !broker.enabled || try(var.environments[id].kind != "pr", false)])
    error_message = "Enabled brokers require an existing non-PR environment."
  }
  validation {
    condition     = alltrue([for id, broker in var.credential_brokers : !broker.enabled || can(regex("^sha256:[a-f0-9]{64}$", local.broker_image_digest))])
    error_message = "Enabled brokers need an approved image digest in enterprise/infra/gcp/environments/broker-image.json."
  }
  validation {
    condition     = alltrue([for id, broker in var.credential_brokers : !broker.enabled || (var.byok_project_id != "" && broker.byok_project_id == var.byok_project_id)])
    error_message = "Enabled brokers must use the environment's configured Secret Store project."
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
}
// Wire IAM after both modules exist: broker -> worker settings must not create a
// reverse module dependency through the worker's service-account output.
resource "google_kms_crypto_key_iam_member" "broker_signer" {
  for_each      = module.credential_broker
  crypto_key_id = each.value.signing_key
  role          = "roles/cloudkms.signer"
  member        = "serviceAccount:${module.environment[each.key].worker_service_account}"
}
output "credential_brokers" { value = { for id, broker in module.credential_broker : id => broker.worker_configuration } }
output "managed_credential_brokers" {
  description = "Non-secret applied broker intent, including disabled entries, for controller reconciliation."
  value       = { for id, broker in var.credential_brokers : id => broker if contains(keys(var.environments), id) }
}
