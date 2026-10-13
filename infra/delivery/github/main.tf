locals {
  repository_name = split("/", var.repository)[1]
  nonproduction   = try(jsondecode(var.nonproduction_config_json), null)
  production      = try(jsondecode(var.production_config_json), null)
  bot_login       = try("${var.release_app.slug}[bot]", null)
  environments = {
    nonproduction            = var.automation_branch
    production               = "main"
    release-automation       = var.automation_branch
    deployment-notifications = var.automation_branch
    certificate-renewal      = var.automation_branch
  }
  # Only public controller/account configuration is allowed in repository
  # variables/state. Runtime secrets and notification/App/DNS credentials have
  # no input or resource in this stack.
  config_keys = toset([
    "project_id", "byok_project_id", "state_bucket", "release_bucket", "deploy_service_account",
    "cloudflare_zone_id", "domain", "repository", "develop_branch", "node_count",
    "deployment_tier", "automation_branch", "trusted_branch", "notification_bucket",
    "notification_service_account", "release_flow_enabled", "release_app_bot_login", "preview_tls_secret",
    "acme_state_secret", "gke_backup_enabled", "gcip_tenants", "gcip_web_api_key",
    "nonproduction_project_id", "nonproduction_state_bucket", "production_origin", "staging_release_bucket",
    "staging_project_id", "staging_max_age_seconds", "preview_request_interval_seconds", "max_preview_environments",
  ])
  public_gcip_valid = try(alltrue(flatten([for config in [local.nonproduction, local.production] : [
    for pool in values(try(config.gcip_tenants, {})) :
    length(setsubtract(toset(keys(pool)), toset(["tenant", "password_sign_up", "google_client_id", "oidc", "saml", "sign_in_domains", "mfa"]))) == 0 &&
    alltrue([for provider in values(try(pool.oidc, {})) : length(setsubtract(toset(keys(provider)), toset(["client_id", "issuer"]))) == 0]) &&
    alltrue([for provider in values(try(pool.saml, {})) : length(setsubtract(toset(keys(provider)), toset(["idp_entity_id", "sso_url", "sp_entity_id", "certificates"]))) == 0]) &&
    length(setsubtract(toset(keys(try(pool.mfa, {}))), toset(["state", "totp"]))) == 0 &&
    length(setsubtract(toset(keys(try(pool.mfa.totp, {}))), toset(["adjacent_intervals"]))) == 0
  ]])), false)
  disabled_flags = {
    GCP_ENVIRONMENTS_ENABLED             = tostring(var.enablement.nonproduction)
    GCP_PRODUCTION_ENABLED               = tostring(var.enablement.production)
    GCP_DEPLOYMENT_NOTIFICATIONS_ENABLED = tostring(var.enablement.notifications)
    GCP_CERTIFICATE_RENEWAL_ENABLED      = tostring(var.enablement.certificates)
  }
  account_inputs_complete = (
    var.nonproduction_config_json != null && var.production_config_json != null &&
    var.nonproduction_bootstrap != null && var.production_bootstrap != null && var.release_app != null
  )
  repository_variables = merge(local.disabled_flags, !var.configure_variables || !local.account_inputs_complete ? {} : merge({
    AIDASH_GCP_CONFIG                           = var.nonproduction_config_json
    AIDASH_GCP_PRODUCTION_CONFIG                = var.production_config_json
    GCP_WORKLOAD_IDENTITY_PROVIDER              = var.nonproduction_bootstrap.workload_identity_provider
    GCP_DEPLOY_SERVICE_ACCOUNT                  = var.nonproduction_bootstrap.deploy_service_account
    GCP_PUBLISH_SERVICE_ACCOUNT                 = var.nonproduction_bootstrap.publish_service_account
    GCP_NOTIFICATION_SERVICE_ACCOUNT            = var.nonproduction_bootstrap.notification_service_account
    GCP_PRODUCTION_WORKLOAD_IDENTITY_PROVIDER   = var.production_bootstrap.workload_identity_provider
    GCP_PRODUCTION_DEPLOY_SERVICE_ACCOUNT       = var.production_bootstrap.deploy_service_account
    GCP_PRODUCTION_NOTIFICATION_SERVICE_ACCOUNT = var.production_bootstrap.notification_service_account
    RELEASE_APP_CLIENT_ID                       = var.release_app.client_id
    GCP_ENVIRONMENTS_ENABLED                    = tostring(var.enablement.nonproduction)
    GCP_PRODUCTION_ENABLED                      = tostring(var.enablement.production)
    GCP_DEPLOYMENT_NOTIFICATIONS_ENABLED        = tostring(var.enablement.notifications)
    GCP_CERTIFICATE_RENEWAL_ENABLED             = tostring(var.enablement.certificates)
    }, var.nonproduction_bootstrap.preview_tls_renewal == null ? {} : {
    GCP_TLS_RENEWAL_WORKLOAD_IDENTITY_PROVIDER = var.nonproduction_bootstrap.preview_tls_renewal.workload_identity_provider
    GCP_TLS_RENEWAL_SERVICE_ACCOUNT            = var.nonproduction_bootstrap.preview_tls_renewal.service_account
    }, var.acme_email == "" ? {} : {
    AIDASH_ACME_EMAIL = var.acme_email
  }))
  checks = {
    main    = "refs/heads/main"
    develop = "refs/heads/${var.automation_branch}"
  }
  # Fixed names distinguish this narrow stack from the repository's broad
  # existing protections. Never import a general baseline ruleset into these.
  ruleset_names = {
    main    = "Aidash delivery main checks"
    develop = "Aidash delivery develop checks"
  }
}

data "github_repository" "aidash" {
  full_name = var.repository
}

data "github_app" "actions" {
  slug = "github-actions"
}

data "github_app" "release" {
  count = var.configure_variables && var.release_app != null ? 1 : 0
  slug  = var.release_app.slug
}

resource "terraform_data" "contract" {
  lifecycle {
    precondition {
      condition     = !var.configure_variables || local.account_inputs_complete
      error_message = "Account configuration phase requires both complete bootstrap outputs, both public controller JSON inputs and the installed release App metadata."
    }
    precondition {
      condition     = data.github_repository.aidash.default_branch == var.automation_branch && data.github_repository.aidash.name == local.repository_name
      error_message = "The configured execution branch must be the existing repository default; this stack does not rename branches or adopt another repository."
    }
    precondition {
      condition     = !var.configure_variables || try(tonumber(data.github_app.release[0].id) == var.release_app.id && tonumber(data.github_app.actions.id) > 0, false)
      error_message = "The supplied release App ID must match the actual App slug."
    }
    precondition {
      condition = !var.configure_variables || try(alltrue([for config in [local.nonproduction, local.production] :
        config.repository == var.repository && config.domain == "aidash.run" &&
        config.develop_branch == var.automation_branch && config.automation_branch == var.automation_branch &&
        config.trusted_branch == "main" && config.release_app_bot_login == local.bot_login &&
        length(setsubtract(toset(keys(config)), local.config_keys)) == 0 && local.public_gcip_valid &&
        length(setsubtract(toset(keys(try(config.node_count, {}))), toset(config.deployment_tier == "production" ? ["production"] : ["develop", "test", "pr"]))) == 0 &&
        alltrue([for count in values(try(config.node_count, {})) : count >= 1 && floor(count) == count]) &&
        can(regex("^[a-f0-9]{32}$", config.cloudflare_zone_id)) &&
        (config.deployment_tier != "production" || !contains(keys(config), "acme_state_secret")) &&
        (!contains(keys(config), "gke_backup_enabled") || (config.deployment_tier == "production" && contains([true, false], try(config.gke_backup_enabled, false))))
      ]), false)
      error_message = "Controller JSON must contain only allowed non-secret fields and match the repository, exact execution branch, trusted main and release App."
    }
    precondition {
      condition = !var.configure_variables || try(
        local.nonproduction.deployment_tier == "nonproduction" && local.production.deployment_tier == "production" &&
        local.nonproduction.project_id == var.nonproduction_bootstrap.project_id && local.production.project_id == var.production_bootstrap.project_id &&
        local.nonproduction.project_id != local.production.project_id &&
        alltrue([for key in ["state_bucket", "release_bucket", "notification_bucket", "deploy_service_account", "notification_service_account"] :
          local.nonproduction[key] == var.nonproduction_bootstrap[key] && local.production[key] == var.production_bootstrap[key]
        ]) &&
        local.production.nonproduction_project_id == local.nonproduction.project_id &&
        local.production.nonproduction_state_bucket == local.nonproduction.state_bucket &&
        local.production.staging_project_id == local.nonproduction.project_id &&
        local.production.staging_release_bucket == local.nonproduction.release_bucket &&
        length(toset([local.nonproduction.state_bucket, local.nonproduction.release_bucket, local.nonproduction.notification_bucket,
        local.production.state_bucket, local.production.release_bucket, local.production.notification_bucket])) == 6,
      false)
      error_message = "Controller identities must match bootstrap outputs, and production/nonproduction projects and all state/release/notification buckets must be isolated."
    }
    precondition {
      condition     = !var.require_staging_check || (var.configure_variables && var.bootstrap_code_installed && var.release_app != null)
      error_message = "Install and review the privileged main CD code before requiring App staging checks for every main-target PR."
    }
    precondition {
      condition     = !anytrue(values(var.enablement)) || (var.configure_variables && var.bootstrap_code_installed && var.activation_validated)
      error_message = "Enablement requires explicit installed-code and controlled activation acknowledgements; all defaults are disabled."
    }
    precondition {
      condition     = !var.enablement.production || (var.enablement.nonproduction && var.require_staging_check)
      error_message = "Production activation requires nonproduction and the App-bound staging merge gate."
    }
    precondition {
      condition     = !var.enablement.notifications || var.cloudflare_account_id != ""
      error_message = "Configure the explicit notification account before controlled mail activation."
    }
    precondition {
      condition     = !var.enablement.certificates || try(var.nonproduction_bootstrap.preview_tls_renewal != null && var.acme_email != "", false)
      error_message = "Certificate renewal activation requires the dedicated bootstrap WIF identity and administrative ACME email."
    }
    precondition {
      condition     = length(setsubtract(var.existing_repository_variables, toset(keys(local.repository_variables)))) == 0 && (!var.existing_notification_account_variable || var.cloudflare_account_id != "")
      error_message = "Adoption must use only fixed managed variable names and may not silently delete an existing account variable."
    }
  }
}

resource "github_repository_environment" "delivery" {
  for_each            = local.environments
  repository          = local.repository_name
  environment         = each.key
  can_admins_bypass   = false
  prevent_self_review = each.key == "production" ? var.production_prevent_self_review : false
  dynamic "reviewers" {
    for_each = each.key == "production" ? [var.production_reviewer_user_ids] : []
    content {
      users = reviewers.value
    }
  }
  deployment_branch_policy {
    protected_branches     = false
    custom_branch_policies = true
  }
  lifecycle { prevent_destroy = true }
  depends_on = [terraform_data.contract]
}

resource "github_repository_environment_deployment_policy" "exact_branch" {
  for_each       = local.environments
  repository     = local.repository_name
  environment    = github_repository_environment.delivery[each.key].environment
  branch_pattern = each.value
  lifecycle { prevent_destroy = true }
}

resource "github_repository_ruleset" "delivery_checks" {
  for_each    = local.checks
  name        = local.ruleset_names[each.key]
  repository  = local.repository_name
  target      = "branch"
  enforcement = "active"
  conditions {
    ref_name {
      include = [each.value]
      exclude = []
    }
  }
  rules {
    required_status_checks {
      # Preserve baseline semantics. Existing broad rulesets/classic protection
      # continue to provide PR/review/non-FF policy; this adds only named checks.
      strict_required_status_checks_policy = false
      do_not_enforce_on_create             = false
      required_check {
        context        = "CI Success"
        integration_id = tonumber(data.github_app.actions.id)
      }
      dynamic "required_check" {
        for_each = each.key == "main" && var.require_staging_check && var.release_app != null ? [var.release_app.id] : []
        content {
          context        = "Staging deployment"
          integration_id = required_check.value
        }
      }
    }
  }
  lifecycle { prevent_destroy = true }
  depends_on = [terraform_data.contract]
}

resource "github_actions_variable" "delivery" {
  for_each      = local.repository_variables
  repository    = local.repository_name
  variable_name = each.key
  value         = each.value
  lifecycle { prevent_destroy = true }
  depends_on = [terraform_data.contract]
}

resource "github_actions_environment_variable" "notification_account" {
  count         = !var.configure_variables || var.cloudflare_account_id == "" ? 0 : 1
  repository    = local.repository_name
  environment   = github_repository_environment.delivery["deployment-notifications"].environment
  variable_name = "CLOUDFLARE_ACCOUNT_ID"
  value         = var.cloudflare_account_id
  lifecycle { prevent_destroy = true }
}
