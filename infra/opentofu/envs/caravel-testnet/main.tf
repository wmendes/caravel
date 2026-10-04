# Lane #1's Google Cloud project (DEC-046, DEC-114): one VM for every node,
# its address and firewall, the state bucket and the spending limit. The
# values match what was made by hand before M0.8, so importing them changes
# nothing (D-05).

locals {
  project         = "caravel-testnet"
  project_number  = "771105789348"
  billing_account = "019CC0-F55931-51DBCE"
  zone            = "us-central1-a"
}

module "host" {
  source  = "../../modules/caravel-host-gcp"
  project = local.project
  zone    = local.zone
  name    = "caravel-1"

  # Names and scope from before this module.
  address_name    = "caravel-ip"
  web_rule_name   = "allow-web"
  ssh_rule_name   = "allow-ssh-from-iap"
  ssh_rule_on_tag = false
  # Set up by hand (lanes/perps/deploy/testnet/provision.sh) until lane #1 moves to Docker (D-06).
  install_docker = false
}

module "billing_cap" {
  source          = "../../modules/billing-cap"
  project         = local.project
  project_number  = local.project_number
  billing_account = local.billing_account
  amount          = 100
  currency        = "BRL"
  source_dir      = "${path.root}/../../../gcp/billing-cap"
  # What gcloud uploaded when infra/gcp/setup-billing-cap.sh deployed it.
  existing_source = {
    bucket = "gcf-v2-sources-${local.project_number}-us-central1"
    object = "stop-billing/function-source.zip"
  }
}

# This root's own state. Versioned, private, and never destroyed by a plan.
resource "google_storage_bucket" "state" {
  name                        = "caravel-testnet-tofu-state"
  location                    = "US-CENTRAL1"
  storage_class               = "STANDARD"
  uniform_bucket_level_access = true
  public_access_prevention    = "enforced"

  versioning {
    enabled = true
  }

  lifecycle {
    prevent_destroy = true
  }
}
