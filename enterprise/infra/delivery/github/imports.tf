# SPDX-FileCopyrightText: 2026 kent8192
# SPDX-License-Identifier: BUSL-1.1
# See enterprise/LICENSE.

# Read-only discovery must fill these adoption inputs before planning an
# existing installation. Import blocks adopt only declared objects; no search
# by pattern, repository recreation, or unrelated-policy deletion is performed.
import {
  for_each = var.existing_delivery_ruleset_ids
  to       = github_repository_ruleset.delivery_checks[each.key]
  id       = "${local.repository_name}:${each.value}"
}

import {
  for_each = var.existing_environments
  to       = github_repository_environment.delivery[each.value]
  id       = "${local.repository_name}:${each.value}"
}

import {
  for_each = var.existing_deployment_policy_ids
  to       = github_repository_environment_deployment_policy.exact_branch[each.key]
  id       = "${local.repository_name}:${each.key}:${each.value}"
}

import {
  for_each = var.existing_repository_variables
  to       = github_actions_variable.delivery[each.value]
  id       = "${local.repository_name}:${each.value}"
}

import {
  for_each = var.existing_notification_account_variable ? toset(["account"]) : toset([])
  to       = github_actions_environment_variable.notification_account[0]
  id       = "${local.repository_name}:deployment-notifications:CLOUDFLARE_ACCOUNT_ID"
}
