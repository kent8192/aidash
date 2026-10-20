mock_provider "github" {
  mock_data "github_repository" {
    defaults = {
      name           = "aidash"
      default_branch = "develop/0.1.0"
    }
  }
}

override_data {
  target = data.github_app.actions
  values = { id = "15368" }
}

variables {
  configure_variables          = true
  release_app                  = { id = 543210, client_id = "Iv23.fixture", slug = "aidash-delivery", installation_id = 101010 }
  production_reviewer_user_ids = [51869472]
  cloudflare_account_id        = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
  acme_email                   = "acme@example.invalid"
  nonproduction_bootstrap = {
    project_id                   = "aidash-nonproduction"
    state_bucket                 = "aidash-nonproduction-state"
    release_bucket               = "aidash-nonproduction-releases"
    notification_bucket          = "aidash-nonproduction-notifications"
    workload_identity_provider   = "projects/111111111111/locations/global/workloadIdentityPools/aidash-github/providers/github"
    deploy_service_account       = "aidash-deploy@aidash-nonproduction.iam.gserviceaccount.com"
    publish_service_account      = "aidash-publish@aidash-nonproduction.iam.gserviceaccount.com"
    notification_service_account = "aidash-notification-reader@aidash-nonproduction.iam.gserviceaccount.com"
    preview_tls_renewal = {
      secret_id                  = "aidash-preview-wildcard-tls"
      acme_state_secret_id       = "aidash-preview-acme-state"
      service_account            = "aidash-tls-renewal@aidash-nonproduction.iam.gserviceaccount.com"
      workload_identity_provider = "projects/111111111111/locations/global/workloadIdentityPools/aidash-preview-tls/providers/github"
    }
  }
  production_bootstrap = {
    project_id                   = "aidash-production"
    state_bucket                 = "aidash-production-state"
    release_bucket               = "aidash-production-releases"
    notification_bucket          = "aidash-production-notifications"
    workload_identity_provider   = "projects/222222222222/locations/global/workloadIdentityPools/aidash-github/providers/github"
    deploy_service_account       = "aidash-deploy@aidash-production.iam.gserviceaccount.com"
    notification_service_account = "aidash-notification-reader@aidash-production.iam.gserviceaccount.com"
  }
  nonproduction_config_json = jsonencode({
    project_id                   = "aidash-nonproduction"
    state_bucket                 = "aidash-nonproduction-state"
    release_bucket               = "aidash-nonproduction-releases"
    notification_bucket          = "aidash-nonproduction-notifications"
    deploy_service_account       = "aidash-deploy@aidash-nonproduction.iam.gserviceaccount.com"
    notification_service_account = "aidash-notification-reader@aidash-nonproduction.iam.gserviceaccount.com"
    repository                   = "kent8192/aidash"
    domain                       = "aidash.run"
    develop_branch               = "develop/0.1.0"
    automation_branch            = "develop/0.1.0"
    trusted_branch               = "main"
    deployment_tier              = "nonproduction"
    release_app_bot_login        = "aidash-delivery[bot]"
    release_flow_enabled         = true
    cloudflare_zone_id           = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
    preview_tls_secret           = "aidash-preview-wildcard-tls"
  })
  production_config_json = jsonencode({
    project_id                   = "aidash-production"
    state_bucket                 = "aidash-production-state"
    release_bucket               = "aidash-production-releases"
    notification_bucket          = "aidash-production-notifications"
    deploy_service_account       = "aidash-deploy@aidash-production.iam.gserviceaccount.com"
    notification_service_account = "aidash-notification-reader@aidash-production.iam.gserviceaccount.com"
    repository                   = "kent8192/aidash"
    domain                       = "aidash.run"
    develop_branch               = "develop/0.1.0"
    automation_branch            = "develop/0.1.0"
    trusted_branch               = "main"
    deployment_tier              = "production"
    release_app_bot_login        = "aidash-delivery[bot]"
    release_flow_enabled         = true
    cloudflare_zone_id           = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
    production_origin            = "https://aidash.run"
    nonproduction_project_id     = "aidash-nonproduction"
    nonproduction_state_bucket   = "aidash-nonproduction-state"
    staging_project_id           = "aidash-nonproduction"
    staging_release_bucket       = "aidash-nonproduction-releases"
  })
}

run "protection_phase_needs_no_cloud_or_release_app" {
  command = plan
  variables {
    configure_variables       = false
    nonproduction_bootstrap   = null
    production_bootstrap      = null
    nonproduction_config_json = null
    production_config_json    = null
    release_app               = null
  }
  assert {
    condition = (
      length(github_repository_environment.delivery) == 5 &&
      length(github_actions_variable.delivery) == 4 &&
      alltrue([for item in github_actions_variable.delivery : item.value == "false"]) &&
      length(github_actions_environment_variable.notification_account) == 0
    )
    error_message = "Initial protection must work without cloud/App credentials and touch only disabled flags, not existing account variables."
  }
}

run "disabled_setup_scopes_credentials_to_exact_branches" {
  command = plan
  assert {
    condition = (
      length(github_repository_environment.delivery) == 5 &&
      alltrue([for env in github_repository_environment.delivery :
        !env.can_admins_bypass && !env.deployment_branch_policy[0].protected_branches && env.deployment_branch_policy[0].custom_branch_policies
      ]) &&
      github_repository_environment_deployment_policy.exact_branch["production"].branch_pattern == "main" &&
      alltrue([for name in ["nonproduction", "release-automation", "deployment-notifications", "certificate-renewal"] :
        github_repository_environment_deployment_policy.exact_branch[name].branch_pattern == "develop/0.1.0"
      ])
    )
    error_message = "Only exact trusted execution refs may enter the protected credential environments."
  }
  assert {
    condition = (
      github_repository_environment.delivery["production"].prevent_self_review &&
      toset(github_repository_environment.delivery["production"].reviewers[0].users) == toset([51869472]) &&
      alltrue([for key in ["GCP_ENVIRONMENTS_ENABLED", "GCP_PRODUCTION_ENABLED", "GCP_DEPLOYMENT_NOTIFICATIONS_ENABLED", "GCP_CERTIFICATE_RENEWAL_ENABLED"] :
        github_actions_variable.delivery[key].value == "false"
      ])
    )
    error_message = "Initial setup must retain human production review and all workflow disablement flags."
  }
  assert {
    condition = (
      github_actions_environment_variable.notification_account[0].environment == "deployment-notifications" &&
      github_actions_environment_variable.notification_account[0].value == "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" &&
      github_actions_variable.delivery["GCP_TLS_RENEWAL_SERVICE_ACCOUNT"].value == "aidash-tls-renewal@aidash-nonproduction.iam.gserviceaccount.com" &&
      github_actions_variable.delivery["GCP_PRODUCTION_DEPLOY_SERVICE_ACCOUNT"].value == "aidash-deploy@aidash-production.iam.gserviceaccount.com"
    )
    error_message = "Bootstrap identities and the mail account must route to their intended protected scope."
  }
}

run "narrow_ci_rulesets_preserve_baseline_policy_and_bootstrap_access" {
  command = plan
  assert {
    condition = alltrue([for rule in github_repository_ruleset.delivery_checks :
      rule.enforcement == "active" && length(rule.bypass_actors) == 0 &&
      length(rule.rules[0].pull_request) == 0 && length(rule.rules[0].required_status_checks[0].required_check) == 1 &&
      alltrue([for check in rule.rules[0].required_status_checks[0].required_check : check.context == "CI Success" && check.integration_id == 15368])
    ])
    error_message = "Owned additive rulesets may add only App-bound CI; never replace broad baseline PR policy or block bootstrap on staging."
  }
  assert {
    condition = (
      keys(github_repository_ruleset.delivery_checks) == ["main"] &&
      github_repository_ruleset.delivery_checks["main"].conditions[0].ref_name[0].include == tolist(["refs/heads/main"])
    )
    error_message = "Required checks must target exactly stable main; development branches keep only the baseline ruleset."
  }
}

run "reviewed_rollout_binds_staging_to_release_app" {
  command = plan
  variables {
    require_staging_check    = true
    bootstrap_code_installed = true
  }
  assert {
    condition = (
      length(github_repository_ruleset.delivery_checks["main"].rules[0].required_status_checks[0].required_check) == 2 &&
      length([for check in github_repository_ruleset.delivery_checks["main"].rules[0].required_status_checks[0].required_check : check if check.context == "Staging deployment" && check.integration_id == 543210]) == 1
    )
    error_message = "Only the reviewed release App may satisfy staging."
  }
}

run "incomplete_account_phase_is_rejected" {
  command = plan
  variables {
    configure_variables       = true
    nonproduction_bootstrap   = null
    production_bootstrap      = null
    nonproduction_config_json = null
    production_config_json    = null
    release_app               = null
  }
  expect_failures = [terraform_data.contract]
}

run "activation_requires_explicit_live_gate" {
  command = plan
  variables {
    enablement = { nonproduction = true }
  }
  expect_failures = [terraform_data.contract]
}

run "staging_gate_before_bootstrap_is_rejected" {
  command = plan
  variables {
    require_staging_check = true
  }
  expect_failures = [terraform_data.contract]
}

run "wrong_execution_branch_is_rejected" {
  command = plan
  override_data {
    target = data.github_repository.aidash
    values = { name = "aidash", default_branch = "main" }
  }
  expect_failures = [terraform_data.contract]
}

run "secrets_do_not_fit_public_config_contract" {
  command = plan
  variables {
    nonproduction_config_json = jsonencode({
      project_id                   = "aidash-nonproduction"
      state_bucket                 = "aidash-nonproduction-state"
      release_bucket               = "aidash-nonproduction-releases"
      notification_bucket          = "aidash-nonproduction-notifications"
      deploy_service_account       = "aidash-deploy@aidash-nonproduction.iam.gserviceaccount.com"
      notification_service_account = "aidash-notification-reader@aidash-nonproduction.iam.gserviceaccount.com"
      repository                   = "kent8192/aidash"
      domain                       = "aidash.run"
      develop_branch               = "develop/0.1.0"
      automation_branch            = "develop/0.1.0"
      trusted_branch               = "main"
      deployment_tier              = "nonproduction"
      release_app_bot_login        = "aidash-delivery[bot]"
      cloudflare_zone_id           = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
      AIDASH_SECRET_API_KEY        = "must-not-be-a-Terraform-variable"
    })
  }
  expect_failures = [terraform_data.contract]
}

run "production_cannot_activate_without_staging_merge_gate" {
  command = plan
  variables {
    bootstrap_code_installed = true
    activation_validated     = true
    enablement               = { nonproduction = true, production = true }
  }
  expect_failures = [terraform_data.contract]
}

run "controlled_activation_uses_only_explicit_flags" {
  command = plan
  variables {
    bootstrap_code_installed = true
    activation_validated     = true
    require_staging_check    = true
    enablement               = { nonproduction = true, production = true, notifications = true, certificates = true }
  }
  assert {
    condition = alltrue([for key in ["GCP_ENVIRONMENTS_ENABLED", "GCP_PRODUCTION_ENABLED", "GCP_DEPLOYMENT_NOTIFICATIONS_ENABLED", "GCP_CERTIFICATE_RENEWAL_ENABLED"] :
      github_actions_variable.delivery[key].value == "true"
    ])
    error_message = "Explicit validated activation must publish the selected workflow flags without manipulating secrets."
  }
}

run "production_needs_numeric_human_reviewer" {
  command = plan
  variables {
    production_reviewer_user_ids = []
  }
  expect_failures = [var.production_reviewer_user_ids]
}

run "unrelated_adoption_name_is_rejected" {
  command = plan
  variables {
    existing_delivery_ruleset_ids = { unrelated = 23727469 }
  }
  expect_failures = [var.existing_delivery_ruleset_ids]
}


run "public_gcip_pool_metadata_can_be_published" {
  command = plan
  variables {
    nonproduction_config_json = jsonencode(merge(jsondecode(var.nonproduction_config_json), {
      gcip_tenants = { default = { tenant = "public-pool", oidc = { "oidc.example" = { client_id = "public-client", issuer = "https://issuer.example.invalid" } } } }
    }))
  }
  assert {
    condition     = jsondecode(github_actions_variable.delivery["AIDASH_GCP_CONFIG"].value).gcip_tenants.default.oidc["oidc.example"].client_id == "public-client"
    error_message = "Public GCIP pool/IdP metadata is required by the controller and must reach its config."
  }
}
run "nested_idp_client_secrets_cannot_enter_public_variables" {
  command = plan
  variables {
    nonproduction_config_json = jsonencode(merge(jsondecode(var.nonproduction_config_json), {
      gcip_tenants = { default = { tenant = "public-pool", oidc = { "oidc.example" = { client_id = "public-client", issuer = "https://issuer.example.invalid", client_secret = "private-never-published" } } } }
    }))
  }
  expect_failures = [terraform_data.contract]
}
run "tenant_totp_mfa_and_browser_key_can_be_published" {
  command = plan
  variables {
    nonproduction_config_json = jsonencode(merge(jsondecode(var.nonproduction_config_json), {
      gcip_web_api_key = "public-browser-key"
      gcip_tenants     = { default = { tenant = "public-pool", mfa = { state = "mandatory", totp = { adjacent_intervals = 3 } } } }
    }))
  }
  assert {
    condition     = jsondecode(github_actions_variable.delivery["AIDASH_GCP_CONFIG"].value).gcip_tenants.default.mfa.totp.adjacent_intervals == 3 && jsondecode(github_actions_variable.delivery["AIDASH_GCP_CONFIG"].value).gcip_web_api_key == "public-browser-key"
    error_message = "The controller needs the tenant TOTP window and the public browser key from its config."
  }
}
run "unknown_mfa_settings_cannot_enter_public_variables" {
  command = plan
  variables {
    nonproduction_config_json = jsonencode(merge(jsondecode(var.nonproduction_config_json), {
      gcip_tenants = { default = { tenant = "public-pool", mfa = { state = "enabled", totp = { adjacent_intervals = 3, secret = "private-never-published" } } } }
    }))
  }
  expect_failures = [terraform_data.contract]
}
run "bootstrap_broker_intent_reaches_controller_config" {
  command = plan
  variables {
    nonproduction_config_json = jsonencode(merge(jsondecode(var.nonproduction_config_json), {
      byok_project_id = "aidash-byok"
      credential_brokers = { test = {
        enabled                      = true
        byok_project_id              = "aidash-byok"
        secret_prefix                = "aidash-test-cred-"
        broker_service_account_email = "aidash-test-broker@aidash-nonproduction.iam.gserviceaccount.com"
        signing_key_id               = "projects/aidash-nonproduction/locations/us-central1/keyRings/aidash-test-capability/cryptoKeys/capability"
        signing_version              = "2"
        verification_versions        = ["1", "2"]
      } }
    }))
  }
  assert {
    condition     = jsondecode(github_actions_variable.delivery["AIDASH_GCP_CONFIG"].value).credential_brokers.test.enabled && jsondecode(github_actions_variable.delivery["AIDASH_GCP_CONFIG"].value).credential_brokers.test.signing_version == "2"
    error_message = "Without broker intent the controller drains every bootstrap broker and can never enable one."
  }
}
run "broker_image_cannot_enter_public_variables" {
  command = plan
  variables {
    nonproduction_config_json = jsonencode(merge(jsondecode(var.nonproduction_config_json), {
      byok_project_id = "aidash-byok"
      credential_brokers = { test = {
        byok_project_id              = "aidash-byok"
        secret_prefix                = "aidash-test-cred-"
        broker_service_account_email = "aidash-test-broker@aidash-nonproduction.iam.gserviceaccount.com"
        signing_key_id               = "projects/aidash-nonproduction/locations/us-central1/keyRings/aidash-test-capability/cryptoKeys/capability"
        image                        = "attacker.invalid/broker:latest"
      } }
    }))
  }
  expect_failures = [terraform_data.contract]
}
run "cross_tier_broker_is_rejected" {
  command = plan
  variables {
    nonproduction_config_json = jsonencode(merge(jsondecode(var.nonproduction_config_json), {
      byok_project_id = "aidash-byok"
      credential_brokers = { production = {
        byok_project_id              = "aidash-byok"
        secret_prefix                = "aidash-production-cred-"
        broker_service_account_email = "aidash-production-broker@aidash-nonproduction.iam.gserviceaccount.com"
        signing_key_id               = "projects/aidash-nonproduction/locations/us-central1/keyRings/aidash-production-capability/cryptoKeys/capability"
      } }
    }))
  }
  expect_failures = [terraform_data.contract]
}
run "production_pre_update_backup_plan_reaches_controller_config" {
  command = plan
  variables {
    production_config_json = jsonencode(merge(jsondecode(var.production_config_json), {
      gke_backup_enabled     = true
      pre_update_backup_plan = "projects/aidash-production/locations/us-central1/backupPlans/aidash-production-pre-update"
    }))
  }
  assert {
    condition     = jsondecode(github_actions_variable.delivery["AIDASH_GCP_PRODUCTION_CONFIG"].value).pre_update_backup_plan == "projects/aidash-production/locations/us-central1/backupPlans/aidash-production-pre-update"
    error_message = "Without the plan name the production controller refuses every update."
  }
}
run "pre_update_backup_plan_must_belong_to_its_project" {
  command = plan
  variables {
    production_config_json = jsonencode(merge(jsondecode(var.production_config_json), {
      pre_update_backup_plan = "projects/another-project/locations/us-central1/backupPlans/aidash-production-pre-update"
    }))
  }
  expect_failures = [terraform_data.contract]
}
run "nonproduction_cannot_name_a_pre_update_backup_plan" {
  command = plan
  variables {
    nonproduction_config_json = jsonencode(merge(jsondecode(var.nonproduction_config_json), {
      pre_update_backup_plan = "projects/aidash-nonproduction/locations/us-central1/backupPlans/aidash-production-pre-update"
    }))
  }
  expect_failures = [terraform_data.contract]
}
