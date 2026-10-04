# Machines as code

Infrastructure as code for the machines lanes run on (M0.8, DEC-114), for OpenTofu (versions in `versions.json` `opentofu`). Caravel deploys lanes onto machines; these make the machines.

- `modules/caravel-host-gcp`: a VM with a static address, 80/443 open, ssh through IAP, Docker from its startup script.
- `modules/billing-cap`: the project's spending limit (DEC-045).
- `envs/caravel-testnet`: lane #1's project. Its `caravel_vars` output is a `caravel --var-file`.
- `bootstrap-state.sh`: the state bucket, once per project.

The guide is `docs-site/docs/guides/machines-on-gcp.md`; lane #1's runbook is `docs/RUNBOOK.md` §3.2.
