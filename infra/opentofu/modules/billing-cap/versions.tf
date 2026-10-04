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
}
