module "environment" {
  source   = "../modules/environment"
  for_each = var.environments

  gcip_tenants         = var.gcip_tenants
  gcip_idp_secrets     = var.gcip_idp_secrets
  project_id           = var.project_id
  byok_project_id      = var.byok_project_id
  broker               = try(module.credential_broker[each.key].worker_configuration, null)
  environment_id       = each.key
  environment          = each.value
  hostname             = "${each.value.kind == "pr" ? "preview" : each.value.kind}.${var.domain}"
  node_service_account = google_service_account.nodes.email
  // Cluster attributes order node pools and Workload Identity grants after the
  // cluster and its identity pool exist.
  cluster = {
    name          = google_container_cluster.aidash.name
    location      = google_container_cluster.aidash.location
    workload_pool = google_container_cluster.aidash.workload_identity_config[0].workload_pool
  }
}

resource "cloudflare_dns_record" "environment" {
  for_each = { for id, e in var.environments : id => e if e.published && e.running && e.address != null }
  zone_id  = var.cloudflare_zone_id
  name     = module.environment[each.key].hostname
  type     = "A"
  content  = each.value.address
  ttl      = 60
  proxied  = false
  comment  = "Aidash ${each.key}; managed by Terraform"
}

output "environments" {
  value = { for id, m in module.environment : id => {
    namespace              = m.namespace
    node_pool              = m.node_pool
    hostname               = m.hostname
    runtime_secret         = m.runtime_secret
    server_service_account = m.server_service_account
    worker_service_account = m.worker_service_account
    byok_project_id        = m.byok_project_id
    secret_prefix          = m.secret_prefix
    provider_credentials   = m.provider_credentials
    gcip                   = m.gcip
  } }
}

output "managed_configuration" {
  description = "Non-secret last applied intent used to recover after a controller interruption."
  value       = var.environments
}

output "byok_project_id" { value = var.byok_project_id }
output "secret_prefix" { value = { for id, m in module.environment : id => m.secret_prefix } }
