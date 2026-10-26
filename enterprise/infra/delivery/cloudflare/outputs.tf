# SPDX-FileCopyrightText: 2026 kent8192
# SPDX-License-Identifier: BUSL-1.1
# See enterprise/LICENSE.

output "managed_aliases" {
  value = local.aliases
}

output "adopted_rule_ids" {
  description = "Nonsecret existing IDs adopted during plan; no sender token or destination is exposed."
  value       = local.adopted_rule_ids
}

output "configured_sending_id" {
  value = data.cloudflare_email_sending_subdomain.configured.id
}
