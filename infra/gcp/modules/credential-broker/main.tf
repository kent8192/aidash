terraform {
  required_providers {
    google = {
      source  = "hashicorp/google"
      version = "~> 7.0"
    }
  }
}
locals {
  enabled = var.enabled && var.environment_kind != "pr"
  name    = "aidash-${var.environment_id}-credential-broker"
}
data "google_project" "byok" {
  count      = local.enabled ? 1 : 0
  project_id = var.byok_project_id
}
// Human-run bootstrap owns the broker SA and its BYOK read binding. Environment
// automation cannot grant BYOK read access; it only acts as the supplied SA.
resource "google_service_account_iam_member" "deploy" {
  count              = local.enabled ? 1 : 0
  service_account_id = "projects/${var.project_id}/serviceAccounts/${var.broker_service_account_email}"
  role               = "roles/iam.serviceAccountUser"
  member             = "serviceAccount:${var.deploy_service_account}"
}
resource "google_kms_key_ring" "capability" {
  count    = local.enabled ? 1 : 0
  project  = var.project_id
  location = var.region
  name     = "aidash-${var.environment_id}-capability"
}
resource "google_kms_crypto_key" "capability" {
  count    = local.enabled ? 1 : 0
  key_ring = google_kms_key_ring.capability[0].id
  name     = "capability"
  purpose  = "ASYMMETRIC_SIGN"
  version_template {
    algorithm        = "EC_SIGN_ED25519"
    protection_level = "SOFTWARE"
  }
}
resource "google_kms_crypto_key_iam_member" "signer" {
  count         = local.enabled ? 1 : 0
  crypto_key_id = google_kms_crypto_key.capability[0].id
  role          = "roles/cloudkms.signer"
  member        = "serviceAccount:${var.runtime_service_account}"
}
data "google_kms_crypto_key_version" "verification" {
  for_each   = local.enabled ? var.verification_versions : toset([])
  crypto_key = google_kms_crypto_key.capability[0].id
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
    service_account = var.broker_service_account_email
    timeout         = "${var.timeout_secs}s"
    scaling {
      min_instance_count = coalesce(var.min_instances, var.environment_kind == "production" ? 1 : 0)
      max_instance_count = var.max_instances
    }
    // No VPC connector or direct VPC egress: use Cloud Run's default egress.
    containers {
      image = var.image
      ports { container_port = 8080 }
      dynamic "env" {
        for_each = {
          AIDASH_BYOK_PROJECT_NUMBER        = data.google_project.byok[0].number
          AIDASH_PROVIDER_CREDENTIAL_PREFIX = var.secret_prefix
          AIDASH_CAPABILITY_ISSUER          = var.issuer
          AIDASH_CAPABILITY_AUDIENCE        = var.environment_id
          AIDASH_CAPABILITY_PUBLIC_KEYS     = jsonencode({ for v, k in data.google_kms_crypto_key_version.verification : "${google_kms_crypto_key.capability[0].id}/cryptoKeyVersions/${v}" => k.public_key[0].pem })
          AIDASH_BROKER_REQUESTS_PER_SECOND = tostring(var.requests_per_second)
          AIDASH_BROKER_BURST               = tostring(var.burst)
          AIDASH_BROKER_TIMEOUT_SECS        = tostring(var.timeout_secs)
        }
        content {
          name  = env.key
          value = env.value
        }
      }
    }
  }
  depends_on = [google_service_account_iam_member.deploy]
}
output "worker_configuration" {
  value = local.enabled ? {
    endpoint = "${google_cloud_run_v2_service.broker[0].uri}/api/v1"
    issuer   = var.issuer
    audience = var.environment_id
    kid      = "${google_kms_crypto_key.capability[0].id}/cryptoKeyVersions/${var.signing_version}"
  } : null
}
output "service_account" { value = local.enabled ? var.broker_service_account_email : null }
