terraform {
  required_providers {
    google = {
      source  = "hashicorp/google"
      version = "~> 7.0"
    }
  }
}
locals {
  enabled            = var.enabled && var.environment_kind != "pr"
  tenant_secret_role = var.role == "tenant-secrets"
  name               = local.tenant_secret_role ? "aidash-${var.environment_id}-tenant-secret-broker" : "aidash-${var.environment_id}-credential-broker"
  // Leave time to cancel upstream work, return a response and record the audit
  // before Cloud Run closes the request at its platform deadline.
  broker_deadline_secs = var.timeout_secs - 30
  // Provider and Tenant Secret tokens must never share an audience, even though
  // their JWS typ already differs.
  audience = local.tenant_secret_role ? "${var.environment_id}-tsec" : var.environment_id
  public_keys = merge(
    { for v, k in data.google_kms_crypto_key_version.verification : "${var.signing_key_id}/cryptoKeyVersions/${v}" => k.public_key[0].pem },
    { for v, k in data.google_kms_crypto_key_version.secondary_verification : "${var.secondary_signing_key_id}/cryptoKeyVersions/${v}" => k.public_key[0].pem },
  )
  // Each public key names the issuer its tokens must carry. Both the server and
  // the worker may mint Tenant Secret tokens, so one issuer string cannot cover
  // both keys.
  secret_call_issuers = merge(
    { for v, k in data.google_kms_crypto_key_version.verification : "${var.signing_key_id}/cryptoKeyVersions/${v}" => var.issuer },
    { for v, k in data.google_kms_crypto_key_version.secondary_verification : "${var.secondary_signing_key_id}/cryptoKeyVersions/${v}" => var.secondary_issuer },
  )
}
data "google_project" "byok" {
  count      = local.enabled ? 1 : 0
  project_id = var.byok_project_id
}
// Human-run bootstrap owns both broker SAs, their BYOK read bindings and the
// deploy identity's actAs grants. Environment automation holds an IAM deny on the
// broker SAs' setIamPolicy (#151), so this module must not bind anything on them.
resource "google_kms_crypto_key_iam_member" "signer" {
  count         = local.enabled && var.bind_runtime_signer ? 1 : 0
  crypto_key_id = var.signing_key_id
  role          = "roles/cloudkms.signer"
  member        = "serviceAccount:${var.runtime_service_account}"
}
data "google_kms_crypto_key_version" "verification" {
  for_each   = local.enabled ? var.verification_versions : toset([])
  crypto_key = var.signing_key_id
  version    = each.key
}
data "google_kms_crypto_key_version" "secondary_verification" {
  for_each   = local.enabled && var.secondary_signing_key_id != "" ? var.secondary_verification_versions : toset([])
  crypto_key = var.secondary_signing_key_id
  version    = each.key
}
resource "google_cloud_run_v2_service" "broker" {
  count                = local.enabled ? 1 : 0
  project              = var.project_id
  name                 = local.name
  location             = var.region
  ingress              = var.ingress
  invoker_iam_disabled = var.invoker_iam_disabled
  deletion_protection  = false
  template {
    service_account                  = var.broker_service_account_email
    timeout                          = "${var.timeout_secs}s"
    max_instance_request_concurrency = 4
    scaling {
      min_instance_count = coalesce(var.min_instances, var.environment_kind == "production" ? 1 : 0)
      max_instance_count = var.max_instances
    }
    // No VPC connector or direct VPC egress: use Cloud Run's default egress.
    containers {
      image = var.image
      // Bound buffered 16 MiB requests and their JSON/relay copies per instance.
      resources { limits = { cpu = "1", memory = "1Gi" } }
      ports { container_port = 8080 }
      dynamic "env" {
        for_each = merge(
          {
            AIDASH_BYOK_PROJECT_NUMBER        = data.google_project.byok[0].number
            AIDASH_CAPABILITY_PUBLIC_KEYS     = jsonencode(local.public_keys)
            AIDASH_BROKER_REQUESTS_PER_SECOND = tostring(var.requests_per_second)
            AIDASH_BROKER_BURST               = tostring(var.burst)
            AIDASH_BROKER_TIMEOUT_SECS        = tostring(local.broker_deadline_secs)
          },
          local.tenant_secret_role ? {
            AIDASH_BROKER_ROLE          = "tenant-secrets"
            AIDASH_TENANT_SECRET_PREFIX = var.secret_prefix
            AIDASH_SECRET_CALL_AUDIENCE = local.audience
            AIDASH_SECRET_CALL_ISSUERS  = jsonencode(local.secret_call_issuers)
            } : {
            AIDASH_PROVIDER_CREDENTIAL_PREFIX = var.secret_prefix
            AIDASH_CAPABILITY_ISSUER          = var.issuer
            AIDASH_CAPABILITY_AUDIENCE        = local.audience
          },
        )
        content {
          name  = env.key
          value = env.value
        }
      }
    }
  }
}
output "worker_configuration" {
  value = local.enabled && !local.tenant_secret_role ? {
    endpoint = "${google_cloud_run_v2_service.broker[0].uri}/api/v1"
    issuer   = var.issuer
    audience = local.audience
    kid      = "${var.signing_key_id}/cryptoKeyVersions/${var.signing_version}"
  } : null
}
output "server_configuration" {
  value = local.enabled && local.tenant_secret_role ? {
    // The server appends /v1/forward itself.
    endpoint = google_cloud_run_v2_service.broker[0].uri
    issuer   = var.issuer
    audience = local.audience
    kid      = "${var.signing_key_id}/cryptoKeyVersions/${var.signing_version}"
  } : null
}
output "service_account" { value = local.enabled ? var.broker_service_account_email : null }
output "signing_key" { value = local.enabled ? var.signing_key_id : null }
