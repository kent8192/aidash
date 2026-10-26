terraform {
  required_version = ">= 1.9, < 2.0"
  required_providers {
    github = {
      source  = "integrations/github"
      version = "6.13.0"
    }
  }
}

# Supply GITHUB_TOKEN only in the provider process environment. Authentication
# and all deployment/mail/DNS/App secrets stay outside Terraform input and state.
provider "github" {
  owner = split("/", var.repository)[0]
}
