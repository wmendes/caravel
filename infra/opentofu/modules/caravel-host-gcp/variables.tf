variable "project" {
  description = "The GCP project."
  type        = string
}

variable "zone" {
  description = "The zone the VM runs in; its region holds the address."
  type        = string
  default     = "us-central1-a"
}

variable "name" {
  description = "The VM's name, which `caravel` reaches over IAP ssh."
  type        = string
}

variable "machine_type" {
  type    = string
  default = "e2-small"
}

variable "image" {
  description = "The boot image when the VM is created. Later image releases never recreate it."
  type        = string
  default     = "ubuntu-os-cloud/ubuntu-2404-lts-amd64"
}

variable "disk_gb" {
  description = "The boot disk's size when the VM is created; grow it later with gcloud, not here."
  type        = number
  default     = 20
}

variable "disk_type" {
  type    = string
  default = "pd-standard"
}

variable "network" {
  type    = string
  default = "default"
}

variable "network_tag" {
  description = "The tag the web firewall rule targets."
  type        = string
  default     = "caravel-web"
}

variable "address_name" {
  description = "The static external address's name (default: <name>-ip)."
  type        = string
  default     = null
}

variable "web_rule_name" {
  description = "The firewall rule for 80/443 (default: <name>-web)."
  type        = string
  default     = null
}

variable "ssh_rule_name" {
  description = "The firewall rule for ssh from IAP (default: <name>-iap-ssh)."
  type        = string
  default     = null
}

variable "ssh_rule_on_tag" {
  description = "Limit the IAP ssh rule to the network tag. False applies it to every VM in the network."
  type        = bool
  default     = true
}

variable "install_docker" {
  description = "Run startup.sh at boot: Docker Engine and Compose (pinned), /opt/caravel for uid 10001, swap."
  type        = bool
  default     = true
}
