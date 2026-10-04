# D-05: what existed before this root, adopted as it is. After the first
# apply these blocks do nothing and can stay as the record of the adoption.

locals {
  sa = "billing-cap@caravel-testnet.iam.gserviceaccount.com"
}

import {
  to = module.host.google_compute_address.this
  id = "projects/caravel-testnet/regions/us-central1/addresses/caravel-ip"
}

import {
  to = module.host.google_compute_firewall.web
  id = "projects/caravel-testnet/global/firewalls/allow-web"
}

import {
  to = module.host.google_compute_firewall.iap_ssh
  id = "projects/caravel-testnet/global/firewalls/allow-ssh-from-iap"
}

import {
  to = module.host.google_compute_instance.this
  id = "projects/caravel-testnet/zones/us-central1-a/instances/caravel-1"
}

import {
  for_each = toset([
    "billingbudgets.googleapis.com", "cloudbilling.googleapis.com", "pubsub.googleapis.com",
    "cloudfunctions.googleapis.com", "run.googleapis.com", "cloudbuild.googleapis.com",
    "artifactregistry.googleapis.com", "eventarc.googleapis.com", "logging.googleapis.com",
  ])
  to = module.billing_cap.google_project_service.api[each.value]
  id = "caravel-testnet/${each.value}"
}

import {
  to = module.billing_cap.google_pubsub_topic.this
  id = "projects/caravel-testnet/topics/billing-cap"
}

import {
  to = module.billing_cap.google_service_account.this
  id = "projects/caravel-testnet/serviceAccounts/${local.sa}"
}

import {
  for_each = toset(["roles/billing.projectManager", "roles/browser"])
  to       = module.billing_cap.google_project_iam_member.role[each.value]
  id       = "caravel-testnet ${each.value} serviceAccount:${local.sa}"
}

import {
  to = module.billing_cap.google_billing_budget.this
  id = "billingAccounts/019CC0-F55931-51DBCE/budgets/9323fb2e-be9d-4364-bb24-163c95063ea6"
}

import {
  to = module.billing_cap.google_cloudfunctions2_function.this
  id = "projects/caravel-testnet/locations/us-central1/functions/stop-billing"
}

import {
  to = module.billing_cap.google_cloud_run_v2_service_iam_member.invoker
  id = "projects/caravel-testnet/locations/us-central1/services/stop-billing roles/run.invoker serviceAccount:${local.sa}"
}

import {
  to = google_storage_bucket.state
  id = "caravel-testnet-tofu-state"
}
