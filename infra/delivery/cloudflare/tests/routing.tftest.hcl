mock_provider "cloudflare" {
  mock_data "cloudflare_zone" {
    defaults = { name = "aidash.run", account = { id = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" }, status = "active" }
  }
  mock_data "cloudflare_email_sending_subdomain" {
    defaults = { id = "cccccccccccccccccccccccccccccccc", name = "aidash.run", enabled = true }
  }
  mock_data "cloudflare_email_routing_settings" {
    defaults = { name = "aidash.run", enabled = true, status = "ready" }
  }
  mock_data "cloudflare_email_routing_address" {
    defaults = { email = "private@example.net", verified = "2026-10-12T00:00:00Z" }
  }
}
variables {
  account_id             = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
  zone_id                = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
  destination_address_id = "dddddddddddddddddddddddddddddddd"
  destination_email      = "private@example.net"
  sending_subdomain_id   = "cccccccccccccccccccccccccccccccc"
}

run "only_fixed_aliases_forward_to_verified_destination" {
  command = plan
  assert {
    condition = (
      toset(keys(cloudflare_email_routing_rule.deployment)) == toset(["prod", "stg", "test"]) &&
      cloudflare_email_routing_rule.deployment["stg"].matchers[0].value == "stg.deployment@aidash.run" &&
      cloudflare_email_routing_rule.deployment["prod"].matchers[0].value == "prod.deployment@aidash.run" &&
      cloudflare_email_routing_rule.deployment["test"].matchers[0].value == "test.deployment@aidash.run" &&
      alltrue([for rule in cloudflare_email_routing_rule.deployment :
        rule.enabled && length(rule.matchers) == 1 && rule.matchers[0].type == "literal" && rule.matchers[0].field == "to" &&
        length(rule.actions) == 1 && rule.actions[0].type == "forward" && length(rule.actions[0].value) == 1 && rule.actions[0].value[0] == "private@example.net"
      ])
    )
    error_message = "Only the three exact deployment aliases may forward to the verified private destination."
  }
}

run "refuse_unverified_destination" {
  command = plan
  override_data {
    target = data.cloudflare_email_routing_address.verified
    values = { email = "private@example.net", verified = null }
  }
  expect_failures = [data.cloudflare_email_routing_address.verified]
}

run "refuse_zone_in_another_account" {
  command = plan
  override_data {
    target = data.cloudflare_zone.existing
    values = { name = "aidash.run", account = { id = "ffffffffffffffffffffffffffffffff" }, status = "active" }
  }
  expect_failures = [data.cloudflare_zone.existing]
}

run "refuse_to_onboard_or_replace_existing_sending" {
  command = plan
  override_data {
    target = data.cloudflare_email_sending_subdomain.configured
    values = { id = "cccccccccccccccccccccccccccccccc", name = "aidash.run", enabled = false }
  }
  expect_failures = [data.cloudflare_email_sending_subdomain.configured]
}

run "refuse_dns_reconfiguration_when_routing_is_not_ready" {
  command = plan
  override_data {
    target = data.cloudflare_email_routing_settings.existing
    values = { name = "aidash.run", enabled = false, status = "misconfigured" }
  }
  expect_failures = [data.cloudflare_email_routing_settings.existing]
}
