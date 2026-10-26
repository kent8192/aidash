mock_provider "cloudflare" {
  mock_data "cloudflare_zone" {
    defaults = { zone_id = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", name = "aidashusercontent.app", account = { id = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" }, status = "active" }
  }
}
variables {
  account_id           = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
  zone_id              = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
  psl_pull_request_url = "https://github.com/publicsuffix/list/pull/1234"
}

run "psl_record_cites_the_pull_request" {
  command = plan
  assert {
    condition = (
      cloudflare_dns_record.psl.name == "_psl.aidashusercontent.app" &&
      cloudflare_dns_record.psl.type == "TXT" &&
      cloudflare_dns_record.psl.content == "\"https://github.com/publicsuffix/list/pull/1234\"" &&
      cloudflare_dns_record.psl.zone_id == "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
    )
    error_message = "The _psl TXT record must hold exactly the quoted publicsuffix/list pull request URL."
  }
}

run "refuse_url_outside_publicsuffix_list" {
  command = plan
  variables {
    psl_pull_request_url = "https://github.com/kent8192/list/pull/1234"
  }
  expect_failures = [var.psl_pull_request_url]
}

run "refuse_zone_in_another_account" {
  command = plan
  override_data {
    target = data.cloudflare_zone.existing
    values = { zone_id = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", name = "aidashusercontent.app", account = { id = "ffffffffffffffffffffffffffffffff" }, status = "active" }
  }
  expect_failures = [data.cloudflare_zone.existing]
}

run "refuse_another_zone" {
  command = plan
  override_data {
    target = data.cloudflare_zone.existing
    values = { zone_id = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", name = "aidash.run", account = { id = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" }, status = "active" }
  }
  expect_failures = [data.cloudflare_zone.existing]
}
