output "topic" {
  value = google_pubsub_topic.this.id
}

output "test_command" {
  description = "A no-op test message; above the budget the same message disables billing."
  value       = "gcloud pubsub topics publish ${var.name} --project=${var.project} --message='{\"costAmount\":1,\"budgetAmount\":${var.amount}}'"
}
