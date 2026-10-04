# The project's spending limit (DEC-045), as infra/gcp/setup-billing-cap.sh
# made it: a monthly budget that alerts at each threshold and a function that
# removes the project's billing account at 100%, which stops everything in the
# project. Google's caveats apply: notifications lag real costs, so this is not
# an exact guarantee, and resources left without billing can be deleted.

locals {
  apis = toset([
    "billingbudgets.googleapis.com", "cloudbilling.googleapis.com", "pubsub.googleapis.com",
    "cloudfunctions.googleapis.com", "run.googleapis.com", "cloudbuild.googleapis.com",
    "artifactregistry.googleapis.com", "eventarc.googleapis.com", "logging.googleapis.com",
  ])
  upload = var.existing_source == null
  source = local.upload ? {
    bucket = var.source_bucket
    object = google_storage_bucket_object.source[0].name
  } : var.existing_source
}

resource "google_project_service" "api" {
  for_each           = local.apis
  project            = var.project
  service            = each.value
  disable_on_destroy = false
}

resource "google_pubsub_topic" "this" {
  project    = var.project
  name       = var.name
  depends_on = [google_project_service.api]
}

resource "google_service_account" "this" {
  project      = var.project
  account_id   = var.name
  display_name = "Caravel billing cap"
}

# Least privilege, on this project only: read the project, remove its billing.
resource "google_project_iam_member" "role" {
  for_each = toset(["roles/billing.projectManager", "roles/browser"])
  project  = var.project
  role     = each.value
  member   = "serviceAccount:${google_service_account.this.email}"
}

resource "google_billing_budget" "this" {
  billing_account = var.billing_account
  display_name    = var.budget_name

  budget_filter {
    projects               = ["projects/${var.project_number}"]
    calendar_period        = "MONTH"
    credit_types_treatment = "INCLUDE_ALL_CREDITS"
  }

  amount {
    specified_amount {
      currency_code = var.currency
      units         = tostring(var.amount)
    }
  }

  dynamic "threshold_rules" {
    for_each = var.thresholds
    content {
      threshold_percent = threshold_rules.value
      spend_basis       = "CURRENT_SPEND"
    }
  }

  all_updates_rule {
    pubsub_topic   = google_pubsub_topic.this.id
    schema_version = "1.0"
  }
}

data "archive_file" "source" {
  count       = local.upload ? 1 : 0
  type        = "zip"
  source_dir  = var.source_dir
  output_path = "${path.root}/.terraform/billing-cap.zip"
  excludes    = ["node_modules"]
}

resource "google_storage_bucket_object" "source" {
  count  = local.upload ? 1 : 0
  bucket = var.source_bucket
  name   = "billing-cap/${data.archive_file.source[0].output_md5}.zip"
  source = data.archive_file.source[0].output_path
}

resource "google_cloudfunctions2_function" "this" {
  project  = var.project
  location = var.region
  name     = "stop-billing"

  build_config {
    runtime     = "nodejs22"
    entry_point = "stopBilling"
    source {
      storage_source {
        bucket = local.source.bucket
        object = local.source.object
      }
    }
  }

  service_config {
    available_memory      = "256Mi"
    max_instance_count    = 1
    ingress_settings      = "ALLOW_INTERNAL_ONLY"
    service_account_email = google_service_account.this.email
    environment_variables = { PROJECT_ID = var.project }
  }

  event_trigger {
    trigger_region        = var.region
    event_type            = "google.cloud.pubsub.topic.v1.messagePublished"
    pubsub_topic          = google_pubsub_topic.this.id
    retry_policy          = "RETRY_POLICY_DO_NOT_RETRY"
    service_account_email = google_service_account.this.email
  }

  depends_on = [google_project_service.api]
}

resource "google_cloud_run_v2_service_iam_member" "invoker" {
  project  = var.project
  location = var.region
  name     = google_cloudfunctions2_function.this.name
  role     = "roles/run.invoker"
  member   = "serviceAccount:${google_service_account.this.email}"
}
