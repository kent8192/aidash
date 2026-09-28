mock_provider "google" {}
mock_provider "cloudflare" {}

variables {
  project_id             = "aidash-fixture"
  release_bucket         = "aidash-fixture-releases"
  deploy_service_account = "deploy@aidash-fixture.iam.gserviceaccount.com"
  cloudflare_zone_id     = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
}

run "empty_has_no_hosts_or_dns" {
  command = plan
  assert {
    condition     = length(module.environment) == 0 && length(cloudflare_dns_record.environment) == 0
    error_message = "An unrequested environment must not be provisioned."
  }
  assert {
    condition     = google_compute_disk.preview_tls.name == "aidash-preview-tls" && google_compute_disk.preview_tls.size == 10
    error_message = "The preview TLS store must survive even when every PR is retired."
  }
}

run "stopped_retains_host_and_disks_without_dns" {
  command = plan
  variables {
    environments = {
      test = {
        kind          = "test"
        incarnation   = "aaaaaaaaaaaa"
        generation    = 1
        running       = false
        published     = false
        spot          = true
        bundle_object = "bundles/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.tar.gz"
        bundle_sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        release_sha   = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
      }
    }
  }
  assert {
    condition     = length(module.environment) == 1 && length(cloudflare_dns_record.environment) == 0
    error_message = "Stopping must retain owned resources while withdrawing DNS."
  }
}

run "no_production" {
  command = plan
  variables {
    environments = {
      production = {
        kind          = "production"
        incarnation   = "aaaaaaaaaaaa"
        generation    = 1
        running       = true
        published     = false
        spot          = false
        bundle_object = "bundles/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.tar.gz"
        bundle_sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        release_sha   = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
      }
    }
  }
  expect_failures = [var.environments]
}

run "no_two_running_previews" {
  command = plan
  variables {
    environments = { for id in ["pr-1", "pr-2"] : id => {
      kind          = "pr"
      incarnation   = "aaaaaaaaaaaa"
      generation    = 1
      running       = true
      published     = false
      spot          = true
      bundle_object = "bundles/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.tar.gz"
      bundle_sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
      release_sha   = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    } }
  }
  expect_failures = [var.environments]
}
