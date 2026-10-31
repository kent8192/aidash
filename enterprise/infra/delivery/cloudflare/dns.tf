# SPDX-FileCopyrightText: 2026 kent8192
# SPDX-License-Identifier: BUSL-1.1
# See enterprise/LICENSE.

data "cloudflare_email_routing_dns" "required" {
  count   = var.configure_routing_dns ? 1 : 0
  zone_id = var.zone_id
}

data "cloudflare_dns_records" "apex" {
  count     = var.configure_routing_dns ? 1 : 0
  zone_id   = var.zone_id
  name      = { exact = var.domain }
  max_items = 1000
}

locals {
  apex_records = var.configure_routing_dns ? data.cloudflare_dns_records.apex[0].result : []
  routing_dns  = var.configure_routing_dns ? data.cloudflare_email_routing_dns.required[0].dns : []
  required_mx  = [for record in local.routing_dns : lower(trimsuffix(record.content, ".")) if record.type == "MX" && contains(["@", var.domain], record.name)]
  required_spf = [for record in local.routing_dns : trim(record.content, "\"") if record.type == "TXT" && contains(["@", var.domain], record.name) && startswith(trim(record.content, "\""), "v=spf1")]
  existing_spf = [for record in local.apex_records : trim(record.content, "\"") if record.type == "TXT" && startswith(trim(record.content, "\""), "v=spf1")]
}

# When already enabled, adopt the Cloudflare-owned configuration rather than
# adding MX/SPF again. DNS conflict guards also run before an initial enable.
import {
  for_each = var.configure_routing_dns && data.cloudflare_email_routing_settings.existing.enabled ? toset([var.zone_id]) : toset([])
  to       = cloudflare_email_routing_dns.setup[0]
  id       = each.value
}

resource "cloudflare_email_routing_dns" "setup" {
  count   = var.configure_routing_dns ? 1 : 0
  zone_id = var.zone_id
  name    = var.domain
  depends_on = [
    data.cloudflare_zone.existing,
    data.cloudflare_email_sending_subdomain.configured,
  ]
  lifecycle {
    prevent_destroy = true
    precondition {
      condition     = length(local.apex_records) < 1000 && length(local.required_mx) > 0 && length(local.required_spf) == 1
      error_message = "Cloudflare must supply a complete Routing DNS proposal before setup; refuse incomplete DNS inventory."
    }
    precondition {
      condition     = alltrue([for record in local.apex_records : record.type != "MX" || contains(local.required_mx, lower(trimsuffix(record.content, ".")))])
      error_message = "Existing root MX belongs to another mail provider. Preserve it; use an explicitly reviewed mail migration before enabling Routing."
    }
    precondition {
      condition     = length(local.existing_spf) <= 1 && alltrue([for content in local.existing_spf : contains(local.required_spf, content)])
      error_message = "Existing root SPF conflicts with Routing. Review and merge the existing policy deliberately; never add a second SPF or overwrite it automatically."
    }
    postcondition {
      condition     = self.enabled && self.status == "ready"
      error_message = "Cloudflare Routing DNS setup must be ready before forwarding rules are applied."
    }
  }
}
