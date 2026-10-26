# SPDX-FileCopyrightText: 2026 kent8192
# SPDX-License-Identifier: BUSL-1.1
# See enterprise/LICENSE.

variable "gcip_tenants" {
  description = "GCIP pool definitions cloned into each environment. Pool aliases are configuration keys, not GCIP-generated IDs. A pool with a tenant has a Tenant Binding; the one pool without a tenant is the Shared Pool."
  type = map(object({
    tenant           = optional(string)
    password_sign_up = optional(bool, true)
    google_client_id = optional(string)
    oidc             = optional(map(object({ client_id = string, issuer = string })), {})
    saml             = optional(map(object({ idp_entity_id = string, sso_url = string, sp_entity_id = string, certificates = list(string) })), {})
    sign_in_domains  = optional(list(string), [])
    # GCIP enforces the MFA Requirement; Aidash never reads second-factor claims as authority.
    # `totp.adjacent_intervals` is the clock-skew window (30-second steps) GCIP accepts around a code.
    mfa = optional(object({
      state = optional(string, "disabled")
      totp  = optional(object({ adjacent_intervals = optional(number, 5) }), {})
    }), {})
  }))
  default = {}
  validation {
    condition     = length(distinct(compact([for pool in values(var.gcip_tenants) : pool.tenant]))) == length(compact([for pool in values(var.gcip_tenants) : pool.tenant])) && alltrue([for alias, pool in var.gcip_tenants : can(regex("^[a-zA-Z0-9_-]+$", alias)) && (pool.tenant == null || can(regex("^[a-zA-Z0-9_.:-]+$", pool.tenant))) && (!(length(pool.oidc) > 0 || length(pool.saml) > 0) || !pool.password_sign_up) && alltrue([for id in keys(pool.oidc) : startswith(id, "oidc.")]) && alltrue([for id in keys(pool.saml) : startswith(id, "saml.")])])
    error_message = "Tenant Bindings must be one-to-one; SSO pools must disable password signup and use oidc./saml. provider IDs."
  }
  validation {
    # A Tenant that brings its own single sign-on has a Tenant Binding, and a
    # Sign-in Domain routes only to a bound pool; every other address signs in
    # through the Shared Pool.
    condition     = length([for pool in values(var.gcip_tenants) : pool if pool.tenant == null]) <= 1 && alltrue([for pool in values(var.gcip_tenants) : pool.tenant != null || (length(pool.oidc) == 0 && length(pool.saml) == 0 && length(pool.sign_in_domains) == 0)])
    error_message = "At most one pool may omit its tenant, as the Shared Pool, and the Shared Pool has no SSO providers or Sign-in Domains."
  }
  validation {
    # The server's URL host parser reads a name whose last label is a number
    # (decimal, or 0x hex) as an IPv4 address and refuses it, so reject it here.
    condition     = length(distinct(flatten([for pool in values(var.gcip_tenants) : pool.sign_in_domains]))) == length(flatten([for pool in values(var.gcip_tenants) : pool.sign_in_domains])) && alltrue([for domain in flatten([for pool in values(var.gcip_tenants) : pool.sign_in_domains]) : length(domain) <= 253 && can(regex("^([a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?\\.)+[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$", domain)) && !can(regex("\\.([0-9]+|0x[0-9a-f]*)$", domain))])
    error_message = "Sign-in Domains must be lowercase ASCII DNS names (punycode for IDN) with at least two labels, not IP addresses, and each domain may route to only one pool."
  }
  validation {
    condition     = alltrue([for pool in values(var.gcip_tenants) : contains(["disabled", "enabled", "mandatory"], pool.mfa.state) && pool.mfa.totp.adjacent_intervals == floor(pool.mfa.totp.adjacent_intervals) && pool.mfa.totp.adjacent_intervals >= 0 && pool.mfa.totp.adjacent_intervals <= 10])
    error_message = "The MFA Requirement must be disabled, enabled or mandatory, and mfa.totp.adjacent_intervals an integer from 0 to 10."
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
