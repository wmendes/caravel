terraform {
  required_version = "~> 1.13.1"
  required_providers {
    google = {
      source  = "hashicorp/google"
      version = "8.5.0"
    }
    archive = {
      source  = "hashicorp/archive"
      version = "2.8.1"
    }
  }

  # Made once by ../../bootstrap-state.sh; this root then manages it too.
  backend "gcs" {
    bucket = "caravel-testnet-tofu-state"
    prefix = "envs/caravel-testnet"
  }
}

provider "google" {
  project = local.project
  # The budget API bills its quota to a project; use this one, not gcloud's default.
  user_project_override = true
  billing_project       = local.project
}
