locals {
  gcip_google = { for alias, pool in var.gcip_tenants : alias => pool if pool.google_client_id != null }
  gcip_oidc   = merge({}, [for alias, pool in var.gcip_tenants : { for id, provider in pool.oidc : "${alias}/${id}" => merge(provider, { pool = alias, provider_id = id }) }]...)
  gcip_saml   = merge({}, [for alias, pool in var.gcip_tenants : { for id, provider in pool.saml : "${alias}/${id}" => merge(provider, { pool = alias, provider_id = id }) }]...)
}
resource "google_identity_platform_tenant" "aidash" {
  for_each              = var.gcip_tenants
  project               = var.project_id
  display_name          = "${local.name}-${each.key}"
  allow_password_signup = each.value.password_sign_up
  client {
    # Federated first sign-in must create its tenant account even when passwords are disabled.
    permissions { disabled_user_signup = false }
  }
  deletion_policy = "DELETE"
}
resource "google_identity_platform_tenant_default_supported_idp_config" "google" {
  for_each      = local.gcip_google
  project       = var.project_id
  tenant        = google_identity_platform_tenant.aidash[each.key].name
  idp_id        = "google.com"
  enabled       = true
  client_id     = each.value.google_client_id
  client_secret = var.gcip_idp_secrets["${each.key}/google.com"]
}
resource "google_identity_platform_tenant_oauth_idp_config" "oidc" {
  for_each      = local.gcip_oidc
  project       = var.project_id
  tenant        = google_identity_platform_tenant.aidash[each.value.pool].name
  name          = each.value.provider_id
  display_name  = each.value.provider_id
  enabled       = true
  issuer        = each.value.issuer
  client_id     = each.value.client_id
  client_secret = var.gcip_idp_secrets[each.key]
}
resource "google_identity_platform_tenant_inbound_saml_config" "saml" {
  for_each     = local.gcip_saml
  project      = var.project_id
  tenant       = google_identity_platform_tenant.aidash[each.value.pool].name
  name         = each.value.provider_id
  display_name = each.value.provider_id
  enabled      = true
  idp_config {
    idp_entity_id = each.value.idp_entity_id
    sso_url       = each.value.sso_url
    dynamic "idp_certificates" {
      for_each = each.value.certificates
      content { x509_certificate = idp_certificates.value }
    }
  }
  sp_config {
    sp_entity_id = each.value.sp_entity_id
    callback_uri = "https://${var.project_id}.firebaseapp.com/__/auth/handler"
  }
}
output "gcip" {
  value = {
    project_id              = var.project_id
    public_origin           = "https://${var.hostname}"
    runtime_service_account = google_service_account.runtime.email
    tenant_ids              = [for pool in google_identity_platform_tenant.aidash : pool.name]
    tenant_bindings         = { for alias, pool in google_identity_platform_tenant.aidash : pool.name => var.gcip_tenants[alias].tenant }
    providers               = { for alias, pool in google_identity_platform_tenant.aidash : pool.name => concat(var.gcip_tenants[alias].password_sign_up ? ["password"] : [], var.gcip_tenants[alias].google_client_id != null ? ["google.com"] : [], keys(var.gcip_tenants[alias].oidc), keys(var.gcip_tenants[alias].saml)) }
    password_sign_up        = [for alias, pool in google_identity_platform_tenant.aidash : pool.name if var.gcip_tenants[alias].password_sign_up]
    # Reconciled by the controller: the Google provider has no tenant MFA arguments.
    mfa = { for alias, pool in google_identity_platform_tenant.aidash : pool.name => var.gcip_tenants[alias].mfa.state }
  }
}
