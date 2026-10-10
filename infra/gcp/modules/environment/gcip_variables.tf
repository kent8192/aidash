variable "gcip_tenants" {
  description = "GCIP pool definitions cloned into each environment. Pool aliases are configuration keys, not GCIP-generated IDs."
  type = map(object({
    tenant           = string
    password_sign_up = optional(bool, true)
    google_client_id = optional(string)
    oidc             = optional(map(object({ client_id = string, issuer = string })), {})
    saml             = optional(map(object({ idp_entity_id = string, sso_url = string, sp_entity_id = string, certificates = list(string) })), {})
    sign_in_domains  = optional(list(string), [])
  }))
  default = {}
  validation {
    condition     = length(distinct([for pool in values(var.gcip_tenants) : pool.tenant])) == length(var.gcip_tenants) && alltrue([for alias, pool in var.gcip_tenants : can(regex("^[a-zA-Z0-9_-]+$", alias)) && can(regex("^[a-zA-Z0-9_.:-]+$", pool.tenant)) && (!(length(pool.oidc) > 0 || length(pool.saml) > 0) || !pool.password_sign_up) && alltrue([for id in keys(pool.oidc) : startswith(id, "oidc.")]) && alltrue([for id in keys(pool.saml) : startswith(id, "saml.")])])
    error_message = "Tenant Bindings must be one-to-one; SSO pools must disable password signup and use oidc./saml. provider IDs."
  }
  validation {
    condition     = length(distinct(flatten([for pool in values(var.gcip_tenants) : pool.sign_in_domains]))) == length(flatten([for pool in values(var.gcip_tenants) : pool.sign_in_domains])) && alltrue([for domain in flatten([for pool in values(var.gcip_tenants) : pool.sign_in_domains]) : length(domain) <= 253 && can(regex("^([a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?\\.)+[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$", domain))])
    error_message = "Sign-in Domains must be lowercase ASCII DNS names (punycode for IDN) with at least two labels, and each domain may route to only one pool."
  }
}
variable "gcip_idp_secrets" {
  description = "OAuth client secrets keyed pool-alias/provider-id. Keep in protected Terraform inputs/state."
  type        = map(string)
  sensitive   = true
  default     = {}
}
