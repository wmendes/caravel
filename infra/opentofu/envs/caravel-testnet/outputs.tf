output "address" {
  value = module.host.address
}

# `tofu output -raw caravel_vars > hosts.vars.toml`, then
# `caravel plan lanes/perps/config/lane.caravel-perps.testnet.toml --env testnet --var-file hosts.vars.toml`.
output "caravel_vars" {
  description = "Lane #1's host, as vars for caravel --var-file (TOML)."
  value       = <<-EOT
    host_address = "${module.host.name}"
    host_project = "${local.project}"
    host_zone = "${local.zone}"
    public_url = "${module.host.public_url}"
  EOT
}

output "billing_cap_test" {
  value = module.billing_cap.test_command
}
