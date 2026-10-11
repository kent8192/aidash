project_id          = "your-aidash-project"
byok_project_id     = "your-aidash-byok-project"
state_bucket_name   = "your-aidash-project-tfstate"
release_bucket_name = "your-aidash-project-releases"
# Opt in after the BYOK/bootstrap prerequisites are configured.
byok_broker_environments = [] # e.g. ["production", "test"]
# Required once byok_broker_environments is set (#151): who receives Credential Broker alerts.
security_alert_emails = [] # e.g. ["security@example.com"]
# GitHub Environments every credentialed workflow job must run in; protect them in repository settings.
# deploy_environment  = "gcp-deploy"         # unattended no-build runs; main-only branch policy
# release_environment = "gcp-deploy-release" # build/publish runs; required reviewers
