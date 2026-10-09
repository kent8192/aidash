// The hostname belongs to the preview slot, not to an individual PR. Keep its
// certificate/account store even when the last PR environment is destroyed.
resource "google_compute_disk" "preview_tls" {
  name   = "aidash-preview-tls"
  zone   = "us-central1-a"
  type   = "pd-balanced"
  size   = 10
  labels = { application = "aidash", lifecycle = "nonproduction", purpose = "preview-tls" }
  lifecycle { prevent_destroy = true }
}

module "environment" {
  source   = "../modules/environment"
  for_each = var.environments

  gcip_tenants           = var.gcip_tenants
  gcip_idp_secrets       = var.gcip_idp_secrets
  project_id             = var.project_id
  environment_id         = each.key
  environment            = each.value
  hostname               = "${each.value.kind == "pr" ? "preview" : each.value.kind}.${var.domain}"
  release_bucket         = var.release_bucket
  deploy_service_account = var.deploy_service_account
  preview_tls_disk       = each.value.kind == "pr" ? google_compute_disk.preview_tls.id : null
}

resource "cloudflare_dns_record" "environment" {
  for_each = { for id, e in var.environments : id => e if e.published && e.running }
  zone_id  = var.cloudflare_zone_id
  name     = module.environment[each.key].hostname
  type     = "A"
  content  = module.environment[each.key].external_ip
  ttl      = 60
  proxied  = false
  comment  = "Aidash ${each.key}; managed by Terraform"
}

output "environments" {
  value = { for id, m in module.environment : id => {
    instance       = m.instance
    zone           = m.zone
    hostname       = m.hostname
    external_ip    = m.external_ip
    runtime_secret = m.runtime_secret
    gcip           = m.gcip
  } }
}

output "managed_configuration" {
  description = "Non-secret last applied intent used to recover after a controller interruption."
  value       = var.environments
}
