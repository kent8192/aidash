# SPDX-FileCopyrightText: 2026 kent8192
# SPDX-License-Identifier: BUSL-1.1
# See enterprise/LICENSE.

output "managed_inventory" {
  description = "Fixed inventory for the read-only discovery/adoption helper; no secret values."
  value = {
    repository                    = var.repository
    environments                  = local.environments
    ruleset_names                 = local.ruleset_names
    repository_variable_names     = sort(keys(local.repository_variables))
    notification_account_variable = !var.configure_variables || var.cloudflare_account_id == "" ? null : "deployment-notifications:CLOUDFLARE_ACCOUNT_ID"
    release_app = var.release_app == null ? null : {
      id              = var.release_app.id
      client_id       = var.release_app.client_id
      slug            = var.release_app.slug
      installation_id = var.release_app.installation_id
      bot_login       = local.bot_login
    }
    configure_variables = var.configure_variables
    enablement          = var.enablement
  }
}
