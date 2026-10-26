# The zone exists because the domain was registered at Cloudflare Registrar.
# This root never creates or deletes the zone; it only adds the records the
# Public Suffix List submission needs (issue #535).
data "cloudflare_zone" "existing" {
  zone_id = var.zone_id
  lifecycle {
    postcondition {
      condition     = self.name == var.domain && self.account.id == var.account_id && self.status == "active"
      error_message = "The supplied zone must be active aidashusercontent.app in the explicit account."
    }
  }
}

# The PSL validates a private-section entry by this TXT record, whose value is
# the URL of the pull request that adds the entry. The PSL keeps checking it
# while the entry is listed, so the record must outlive this change.
# https://github.com/publicsuffix/list/wiki/Guidelines
resource "cloudflare_dns_record" "psl" {
  zone_id = data.cloudflare_zone.existing.zone_id
  name    = "_psl.${var.domain}"
  type    = "TXT"
  ttl     = 3600
  content = "\"${var.psl_pull_request_url}\""
  comment = "Public Suffix List validation (kent8192/aidash#535); keep while listed."
  lifecycle {
    prevent_destroy = true
  }
}
