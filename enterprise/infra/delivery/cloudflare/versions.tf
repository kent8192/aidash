# SPDX-FileCopyrightText: 2026 kent8192
# SPDX-License-Identifier: BUSL-1.1
# See enterprise/LICENSE.

terraform {
  required_version = ">= 1.14, < 2.0"
  required_providers {
    cloudflare = {
      source  = "cloudflare/cloudflare"
      version = "= 5.26.0"
    }
  }
}

# The ephemeral input is omitted from saved plans and state. If null, the
# provider may use CLOUDFLARE_API_TOKEN. Local tfvars must remain private/ignored.
provider "cloudflare" {
  api_token = var.cloudflare_api_token
}
