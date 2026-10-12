deployment_tier            = "production"
project_id                 = "your-aidash-production-project"
byok_project_id            = "your-aidash-production-byok-project"
automation_branch          = "develop/0.1.0"
state_bucket_name          = "your-aidash-production-project-tfstate"
release_bucket_name        = "your-aidash-production-project-releases"
notification_bucket_name   = "your-aidash-production-project-notifications"
nonproduction_project_id   = "your-aidash-project"
nonproduction_state_bucket = "your-aidash-project-tfstate"
staging_release_bucket     = "your-aidash-project-releases"
gcip_enabled               = true
byok_broker_environments   = ["production"]
security_alert_emails      = ["security@example.com"] # Credential Broker misuse alerts (#151)
delivery_configuration = {
  cloudflare_zone_id    = "00000000000000000000000000000000"
  release_app_bot_login = "your-release-app[bot]"
  release_flow_enabled  = false
}
# Bootstrap precedes the cluster; these remain false for that first apply.
# The orchestrator enables the agent on the observed existing cluster first,
# then enables these inputs and applies a second reviewed Terraform plan.
production_backup_agent_enabled    = true
production_backup_enabled          = false
production_monitoring_enabled      = false
production_backup_schedule         = "0 19 * * *" # 04:00 Asia/Tokyo, daily
production_backup_retain_days      = 30
production_backup_delete_lock_days = 7
