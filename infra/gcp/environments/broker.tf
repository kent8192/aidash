variable "credential_brokers" {
  description = "Explicit opt-in only for environments with a Provider Credential Store. Never populate for PR previews."
  type = map(object({
    enabled                      = optional(bool, false)
    byok_project_id              = string
    secret_prefix                = string
    broker_service_account_email = string
    image                        = string
  }))
  default = {}
  validation {
    condition     = alltrue([for id, broker in var.credential_brokers : !broker.enabled || try(var.environments[id].kind != "pr", false)])
    error_message = "Enabled brokers require an existing non-PR environment."
  }
}
module "credential_broker" {
  source   = "../modules/credential-broker"
  for_each = { for id, broker in var.credential_brokers : id => broker if broker.enabled && try(var.environments[id].kind != "pr", false) }

  enabled                      = true
  project_id                   = var.project_id
  byok_project_id              = each.value.byok_project_id
  environment_id               = each.key
  environment_kind             = var.environments[each.key].kind
  secret_prefix                = each.value.secret_prefix
  image                        = each.value.image
  runtime_service_account      = module.environment[each.key].runtime_service_account
  deploy_service_account       = var.deploy_service_account
  broker_service_account_email = each.value.broker_service_account_email
}
output "credential_brokers" { value = { for id, broker in module.credential_broker : id => broker.worker_configuration } }
