# Copy to secrets.tfvars, chmod 600, fill locally. Do not commit real values.
# This token input is ephemeral and absent from saved plans/state.
cloudflare_api_token = "REPLACE_WITH_SCOPED_BOOTSTRAP_TOKEN"

account_id = "REPLACE_WITH_EXACT_32_HEX_ACCOUNT_ID"
zone_id    = "REPLACE_WITH_EXACT_32_HEX_ZONE_ID"
# Leave unset until the publicsuffix/list pull request exists.
# psl_pull_request_url = "https://github.com/publicsuffix/list/pull/REPLACE_WITH_NUMBER"
