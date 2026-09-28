module "environment" {
  source   = "../modules/environment"
  for_each = var.environments

  project_id             = var.project_id
  environment_id         = each.key
  environment            = each.value
  hostname               = "${each.value.kind == "pr" ? "preview" : each.value.kind}.${var.domain}"
  release_bucket         = var.release_bucket
  deploy_service_account = var.deploy_service_account
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
  } }
}

output "managed_configuration" {
  description = "Non-secret last applied intent used to recover after a controller interruption."
  value       = var.environments
}
