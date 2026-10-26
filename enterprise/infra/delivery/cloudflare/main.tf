# SPDX-FileCopyrightText: 2026 kent8192
# SPDX-License-Identifier: BUSL-1.1
# See enterprise/LICENSE.

# Existing zone, sending registration, verified destination and DNS remain
# externally owned. This module never creates/deletes a zone, onboards Sending,
# changes Sending DKIM/DMARC, or triggers destination verification mail.
data "cloudflare_zone" "existing" {
  zone_id = var.zone_id
  lifecycle {
    postcondition {
      condition     = self.name == var.domain && self.account.id == var.account_id && self.status == "active"
      error_message = "The supplied zone must be active aidash.run in the explicit account."
    }
  }
}

locals {
  aliases = {
    stg       = "stg.deployment@aidash.run"
    prod      = "prod.deployment@aidash.run"
    test      = "test.deployment@aidash.run"
    licensing = "licensing@aidash.run"
  }
  # Deployment mail uses its tier in the rule name. The licensing address is a
  # public contact route, not a deployment notification, so it gets its own name.
  rule_names = {
    stg       = "Aidash stg deployment notifications"
    prod      = "Aidash prod deployment notifications"
    test      = "Aidash test deployment notifications"
    licensing = "Aidash licensing inquiries"
  }
}

data "cloudflare_email_sending_subdomain" "configured" {
  zone_id      = var.zone_id
  subdomain_id = var.sending_subdomain_id
  lifecycle {
    postcondition {
      condition     = self.name == var.domain && self.enabled
      error_message = "The existing aidash.run Email Sending registration must already be configured and enabled."
    }
  }
}

data "cloudflare_email_routing_settings" "existing" {
  zone_id = var.zone_id
  lifecycle {
    postcondition {
      condition     = self.name == var.domain && (var.configure_routing_dns || (self.enabled && self.status == "ready"))
      error_message = "Email Routing must already be ready; review DNS conflicts before enabling Routing separately."
    }
  }
}

data "cloudflare_email_routing_address" "verified" {
  account_id                     = var.account_id
  destination_address_identifier = var.destination_address_id
  lifecycle {
    postcondition {
      condition     = self.email == var.destination_email && try(length(self.verified) > 0, false)
      error_message = "The destination must match the supplied private mailbox and already be verified."
    }
  }
}

data "cloudflare_email_routing_rules" "existing" {
  zone_id   = var.zone_id
  max_items = 1000
  lifecycle {
    postcondition {
      condition     = length(self.result) < 1000
      error_message = "Rule inventory reached its limit; refuse incomplete adoption."
    }
  }
}

locals {
  matching_rules = {
    for kind, address in local.aliases : kind => [
      for rule in data.cloudflare_email_routing_rules.existing.result : rule
      if anytrue([for matcher in rule.matchers : matcher.type == "literal" && matcher.field == "to" && matcher.value == address])
    ]
  }
  adopted_rule_ids = {
    for kind, rules in local.matching_rules : kind => rules[0].id if length(rules) == 1
  }
}

# Terraform discovers and imports matching rules before any planned update.
# An existing rule is never shadowed by a duplicate newly-created alias.
import {
  for_each = local.adopted_rule_ids
  to       = cloudflare_email_routing_rule.deployment[each.key]
  id       = "${var.zone_id}/${each.value}"
}

data "cloudflare_email_routing_rule" "adopted" {
  for_each        = local.adopted_rule_ids
  zone_id         = var.zone_id
  rule_identifier = each.value
  lifecycle {
    postcondition {
      condition = (
        contains([null, "api"], self.source) && length(self.actions) == 1 && try(self.actions[0].type == "forward", false) &&
        length(self.matchers) == 1 &&
        try(self.matchers[0].type == "literal" && self.matchers[0].field == "to" && self.matchers[0].value == local.aliases[each.key], false)
      )
      error_message = "Adoption is restricted to exact, API-managed Aidash aliases; do not take over Worker or multi-matcher rules."
    }
  }
}

resource "cloudflare_email_routing_rule" "deployment" {
  for_each = local.aliases
  zone_id  = var.zone_id
  name     = local.rule_names[each.key]
  enabled  = true
  source   = "api"
  matchers = [{ type = "literal", field = "to", value = each.value }]
  actions  = [{ type = "forward", value = [var.destination_email] }]
  depends_on = [
    data.cloudflare_zone.existing,
    data.cloudflare_email_sending_subdomain.configured,
    data.cloudflare_email_routing_address.verified,
    data.cloudflare_email_routing_settings.existing,
    data.cloudflare_email_routing_rule.adopted,
    cloudflare_email_routing_dns.setup,
  ]
  lifecycle {
    prevent_destroy = true
    precondition {
      condition     = length(local.matching_rules[each.key]) <= 1
      error_message = "Multiple existing rules match an Aidash alias; resolve ownership before applying."
    }
  }
}
