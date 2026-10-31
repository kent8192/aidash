# Copy to secrets.tfvars, chmod 600, fill locally. Do not commit real values.
# This token input is ephemeral and absent from saved plans/state.
cloudflare_api_token = "REPLACE_WITH_SCOPED_BOOTSTRAP_TOKEN"

account_id             = "REPLACE_WITH_EXACT_32_HEX_ACCOUNT_ID"
zone_id                = "REPLACE_WITH_EXACT_32_HEX_ZONE_ID"
destination_address_id = "REPLACE_WITH_EXISTING_VERIFIED_DESTINATION_ID"
destination_email      = "REPLACE_WITH_PRIVATE_VERIFIED_EMAIL"
sending_subdomain_id   = "REPLACE_WITH_EXISTING_CONFIGURED_SENDING_ID"

# Enables only safe Routing DNS changes after live read-only conflict checks.
# Leave false when Routing is already ready and its DNS remains externally owned.
configure_routing_dns = false
