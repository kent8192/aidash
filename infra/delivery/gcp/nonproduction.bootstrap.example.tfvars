deployment_tier          = "nonproduction"
project_id               = "your-aidash-project"
byok_project_id          = "your-aidash-byok-project"
automation_branch        = "develop/0.1.0"
state_bucket_name        = "your-aidash-project-tfstate"
release_bucket_name      = "your-aidash-project-releases"
notification_bucket_name = "your-aidash-project-notifications"
gcip_enabled             = true
preview_tls_secret_id    = "aidash-preview-wildcard-tls"
byok_broker_environments = ["develop", "test"]
delivery_configuration = {
  cloudflare_zone_id    = "00000000000000000000000000000000"
  release_app_bot_login = "your-release-app[bot]"
  preview_tls_secret    = "aidash-preview-wildcard-tls"
  release_flow_enabled  = false
}
