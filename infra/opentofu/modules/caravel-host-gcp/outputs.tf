output "name" {
  value = google_compute_instance.this.name
}

output "zone" {
  value = google_compute_instance.this.zone
}

output "address" {
  description = "The static external IPv4 address."
  value       = google_compute_address.this.address
}

output "public_url" {
  description = "An HTTPS name for the address with no DNS to manage (sslip.io), which Caddy gets a certificate for."
  value       = "https://${replace(google_compute_address.this.address, ".", "-")}.sslip.io"
}

output "startup_script" {
  description = "The setup script's path, to run by hand on a VM made before it (sudo bash startup.sh)."
  value       = "${path.module}/startup.sh"
}
