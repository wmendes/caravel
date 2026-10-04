variable "project" {
  description = "The GCP project the cap protects."
  type        = string
}

variable "project_number" {
  description = "Its number; budget filters name projects by number."
  type        = string
}

variable "billing_account" {
  description = "The billing account that holds the budget."
  type        = string
}

variable "region" {
  type    = string
  default = "us-central1"
}

variable "amount" {
  description = "The monthly limit, in whole units of currency."
  type        = number
}

variable "currency" {
  type    = string
  default = "BRL"
}

variable "thresholds" {
  description = "Alerts, as fractions of the limit. Reaching 1.0 detaches billing."
  type        = list(number)
  default     = [0.5, 0.9, 1.0]
}

variable "name" {
  description = "The topic, service account and function share it."
  type        = string
  default     = "billing-cap"
}

variable "budget_name" {
  type    = string
  default = "Caravel testnet monthly limit"
}

variable "source_dir" {
  description = "The function's code (infra/gcp/billing-cap)."
  type        = string
}

variable "source_bucket" {
  description = "A bucket to upload the zipped source to. Leave null when existing_source is set."
  type        = string
  default     = null
}

variable "existing_source" {
  description = "An archive already in Cloud Storage (what gcloud uploaded for a function made before this module); nothing is uploaded."
  type        = object({ bucket = string, object = string })
  default     = null
}
