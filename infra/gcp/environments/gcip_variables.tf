variable "gcip_tenants" {
  description = "GCIP pool definitions cloned into each environment. Pool aliases are configuration keys, not GCIP-generated IDs."
  type = map(object({
    tenant           = string
    password_sign_up = optional(bool, true)
    google_client_id = optional(string)
    oidc             = optional(map(object({ client_id = string, issuer = string })), {})
    saml             = optional(map(object({ idp_entity_id = string, sso_url = string, sp_entity_id = string, certificates = list(string) })), {})
    sign_in_domains  = optional(list(string), [])
    # GCIP enforces the MFA Requirement; Aidash never reads second-factor claims as authority.
    mfa = optional(object({ state = optional(string, "disabled") }), {})
  }))
  default = {}
  validation {
    condition     = length(distinct([for pool in values(var.gcip_tenants) : pool.tenant])) == length(var.gcip_tenants) && alltrue([for alias, pool in var.gcip_tenants : can(regex("^[a-zA-Z0-9_-]+$", alias)) && can(regex("^[a-zA-Z0-9_.:-]+$", pool.tenant)) && (!(length(pool.oidc) > 0 || length(pool.saml) > 0) || !pool.password_sign_up) && alltrue([for id in keys(pool.oidc) : startswith(id, "oidc.")]) && alltrue([for id in keys(pool.saml) : startswith(id, "saml.")])])
    error_message = "Tenant Bindings must be one-to-one; SSO pools must disable password signup and use oidc./saml. provider IDs."
  }
  validation {
    # The server's URL host parser reads a name whose last label is a number
    # (decimal, or 0x hex) as an IPv4 address and refuses it, so reject it here.
    condition     = length(distinct(flatten([for pool in values(var.gcip_tenants) : pool.sign_in_domains]))) == length(flatten([for pool in values(var.gcip_tenants) : pool.sign_in_domains])) && alltrue([for domain in flatten([for pool in values(var.gcip_tenants) : pool.sign_in_domains]) : length(domain) <= 253 && can(regex("^([a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?\\.)+[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$", domain)) && !can(regex("\\.([0-9]+|0x[0-9a-f]*)$", domain))])
    error_message = "Sign-in Domains must be lowercase ASCII DNS names (punycode for IDN) with at least two labels, not IP addresses, and each domain may route to only one pool."
    # Lift only after the web and desktop sign-in flows resolve and enroll second factors (#201).
    condition     = alltrue([for pool in values(var.gcip_tenants) : pool.mfa.state == "disabled"])
    error_message = "Only the disabled MFA Requirement is supported until sign-in clients handle second factors."
  }
  validation {
    condition     = alltrue([for pool in values(var.gcip_tenants) : length(pool.oidc) == 0 && length(pool.saml) == 0 || pool.mfa.state == "disabled"])
    error_message = "SSO pools must disable GCIP MFA; their identity provider performs MFA."
  }
}
variable "gcip_idp_secrets" {
  description = "OAuth client secrets keyed pool-alias/provider-id. Keep in protected Terraform inputs/state."
  type        = map(string)
  sensitive   = true
  default     = {}
}
