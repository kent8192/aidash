deployment_tier          = "nonproduction"
automation_branch        = "develop/0.1.0"
project_id               = "your-aidash-project"
byok_project_id          = "your-aidash-byok-project"
state_bucket_name        = "your-aidash-project-tfstate"
release_bucket_name      = "your-aidash-project-releases"
notification_bucket_name = "your-aidash-project-notifications"
# Opt in after the BYOK/bootstrap prerequisites are configured.
byok_broker_environments = [] # e.g. ["develop", "test"]
# Required once byok_broker_environments is set (#151): who receives Credential Broker alerts.
security_alert_emails = [] # e.g. ["security@example.com"]
# Production bootstrap uses a different project and three different buckets,
# deployment_tier = "production", and these mandatory isolation proofs:
# nonproduction_project_id   = "your-aidash-project"
# nonproduction_state_bucket = "your-aidash-project-tfstate"
# byok_broker_environments   = ["production"]
# staging_release_bucket     = "your-aidash-project-releases"
