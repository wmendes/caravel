# One VM for a lane's nodes (M0.8, DEC-114): a static address, 80/443 open to
# the web, ssh only from IAP's range, no service account. `caravel apply`
# runs the lane on it; this module only makes the machine.

locals {
  region        = join("-", slice(split("-", var.zone), 0, 2))
  address_name  = coalesce(var.address_name, "${var.name}-ip")
  web_rule_name = coalesce(var.web_rule_name, "${var.name}-web")
  ssh_rule_name = coalesce(var.ssh_rule_name, "${var.name}-iap-ssh")
  metadata = merge(
    { enable-oslogin = "TRUE" },
    var.install_docker ? { startup-script = file("${path.module}/startup.sh") } : {},
  )
}

resource "google_compute_address" "this" {
  project      = var.project
  region       = local.region
  name         = local.address_name
  address_type = "EXTERNAL"
  network_tier = "PREMIUM"
}

resource "google_compute_firewall" "web" {
  project       = var.project
  name          = local.web_rule_name
  network       = var.network
  direction     = "INGRESS"
  source_ranges = ["0.0.0.0/0"]
  target_tags   = [var.network_tag]

  dynamic "allow" {
    for_each = ["80", "443"]
    content {
      protocol = "tcp"
      ports    = [allow.value]
    }
  }
}

# IAP's TCP forwarding range (cloud.google.com/iap/docs/using-tcp-forwarding).
resource "google_compute_firewall" "iap_ssh" {
  project       = var.project
  name          = local.ssh_rule_name
  network       = var.network
  direction     = "INGRESS"
  source_ranges = ["35.235.240.0/20"]
  target_tags   = var.ssh_rule_on_tag ? [var.network_tag] : null

  allow {
    protocol = "tcp"
    ports    = ["22"]
  }
}

resource "google_compute_instance" "this" {
  project                   = var.project
  zone                      = var.zone
  name                      = var.name
  machine_type              = var.machine_type
  tags                      = [var.network_tag]
  metadata                  = local.metadata
  allow_stopping_for_update = true

  boot_disk {
    auto_delete = true
    initialize_params {
      image = var.image
      size  = var.disk_gb
      type  = var.disk_type
    }
  }

  network_interface {
    network = var.network
    access_config {
      nat_ip       = google_compute_address.this.address
      network_tier = "PREMIUM"
    }
  }

  shielded_instance_config {
    enable_secure_boot          = true
    enable_vtpm                 = true
    enable_integrity_monitoring = true
  }

  lifecycle {
    # The boot disk holds the lane's stores, and a change here would make a
    # new VM: a new image release is ignored, and a bigger disk is resized in
    # place by hand (gcloud compute disks resize), then disk_gb set to match.
    ignore_changes = [boot_disk[0].initialize_params[0].image, boot_disk[0].initialize_params[0].size]
  }
}
