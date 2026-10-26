terraform {
  required_version = ">= 1.14, < 2.0"
  backend "gcs" {}
  required_providers {
    google = {
      source  = "hashicorp/google"
      version = "~> 7.0"
    }
  }
}

provider "google" {
  project               = var.nonproduction_project_id
  region                = "us-central1"
  user_project_override = true
  billing_project       = var.nonproduction_project_id
}
