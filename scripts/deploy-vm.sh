#!/usr/bin/env bash
# Installs a CI release on the testnet VM and restarts the services (DEC-046).
#
#   gh run download <run id> -n caravel-release-linux-x86_64 -D /tmp/release
#   RELEASE_DIR=/tmp/release ./scripts/deploy-vm.sh             # first time: INIT_KEYS=1
#
# Everything goes through `gcloud compute ssh/scp --tunnel-through-iap`.
# INIT_KEYS=1 copies the validator, relayer and oracle secrets from the local
# Stellar CLI keystore to /opt/caravel/keys on the VM (mode 600, owner
# caravel) and creates the internal API token there. Secrets never enter git.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
RELEASE_DIR="${RELEASE_DIR:?set RELEASE_DIR to the CI release artifact}"
PROJECT="${PROJECT:-caravel-testnet}"
ZONE="${ZONE:-us-central1-a}"
VM="${VM:-caravel-1}"
G=(--zone "$ZONE" --project "$PROJECT" --tunnel-through-iap --quiet)
vm() { gcloud compute ssh "$VM" "${G[@]}" --command "$1"; }
fail() { echo "FAIL: $*" >&2; exit 1; }

echo "== release $(cat "$RELEASE_DIR/COMMIT")"
(cd "$RELEASE_DIR" && sha256sum -c SHA256SUMS 2>/dev/null || shasum -a 256 -c SHA256SUMS)
for pair in "perps_engine:engine_wasm_sha256" "settlement:settlement_wasm_sha256"; do
  have="$(shasum -a 256 "$RELEASE_DIR/contracts/${pair%%:*}.wasm" | awk '{print $1}')"
  [[ "$have" == "$(node -p "require('./versions.json').artifacts.${pair##*:}")" ]] || fail "${pair%%:*}.wasm is not the Wasm of record"
done

IP="$(gcloud compute addresses describe caravel-ip --region="${ZONE%-*}" --project="$PROJECT" --format='value(address)')"
HOST="${IP//./-}.sslip.io"
echo "== host $HOST"

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
mkdir -p "$STAGE/opt/bin" "$STAGE/opt/contracts" "$STAGE/opt/config" "$STAGE/opt/relayer" "$STAGE/opt/relayer-feeds/perps" "$STAGE/opt/web" "$STAGE/systemd"
cp "$RELEASE_DIR/bin/caravel-perps-node" "$STAGE/opt/bin/"
cp "$RELEASE_DIR"/contracts/*.wasm "$STAGE/opt/contracts/"
cp -r "$RELEASE_DIR/relayer/dist" "$RELEASE_DIR/relayer/package.json" "$RELEASE_DIR/relayer/package-lock.json" "$STAGE/opt/relayer/"
cp -r "$RELEASE_DIR/relayer-feeds/perps/dist" "$RELEASE_DIR/relayer-feeds/perps/package.json" "$RELEASE_DIR/relayer-feeds/perps/package-lock.json" "$STAGE/opt/relayer-feeds/perps/"
[[ -d "$RELEASE_DIR/web" ]] && cp -r "$RELEASE_DIR/web/." "$STAGE/opt/web/"
cp lanes/perps/config/lane.caravel-perps.testnet.toml lanes/perps/deploy/testnet/*.toml lanes/perps/deploy/testnet/relayer.json "$STAGE/opt/config/"
cp lanes/perps/deploy/testnet/systemd/* "$STAGE/systemd/"
sed "s/{\$CARAVEL_HOST}/$HOST/" lanes/perps/deploy/testnet/Caddyfile > "$STAGE/Caddyfile"
COPYFILE_DISABLE=1 tar --no-xattrs -czf "$STAGE.tgz" -C "$STAGE" .
gcloud compute scp "${G[@]}" "$STAGE.tgz" "$VM:/tmp/caravel-release.tgz"
rm -f "$STAGE.tgz"

if [[ "${INIT_KEYS:-0}" == "1" ]]; then
  echo "== keys"
  K="$(mktemp -d)"
  trap 'rm -rf "$STAGE" "$K"' EXIT
  for i in 1 2 3; do stellar keys secret "caravel-validator-$i" 2>/dev/null > "$K/validator-$i.key"; done
  {
    echo "CARAVEL_INTERNAL_TOKEN=$(openssl rand -hex 24)"
    echo "CARAVEL_RELAYER_SECRET=$(stellar keys secret caravel-relayer 2>/dev/null)"
    echo "CARAVEL_ORACLE_SECRET=$(stellar keys secret caravel-oracle 2>/dev/null)"
  } > "$K/env"
  chmod 600 "$K"/*
  COPYFILE_DISABLE=1 tar --no-xattrs -czf "$K.tgz" -C "$K" .
  gcloud compute scp "${G[@]}" "$K.tgz" "$VM:/tmp/caravel-keys.tgz"
  rm -f "$K.tgz"
  vm "sudo tar -xzf /tmp/caravel-keys.tgz -C /opt/caravel/keys && rm -f /tmp/caravel-keys.tgz && sudo sh -c 'chown -R caravel:caravel /opt/caravel/keys && chmod 700 /opt/caravel/keys && chmod 600 /opt/caravel/keys/* && ls -l /opt/caravel/keys' | tail -n +2 | awk '{print \$1, \$3, \$9}'"
fi

echo "== install and restart"
vm "set -e
S=\$(mktemp -d); tar -xzf /tmp/caravel-release.tgz -C \$S; rm -f /tmp/caravel-release.tgz
sudo test -f /opt/caravel/keys/env || { echo 'no keys on the VM: run with INIT_KEYS=1'; exit 1; }
sudo systemctl stop caravel-relayer caravel-validator@1 caravel-validator@2 caravel-validator@3 caravel-sequencer 2>/dev/null || true
sudo rsync -a --delete \$S/opt/bin/ /opt/caravel/bin/
sudo rsync -a --delete \$S/opt/contracts/ /opt/caravel/contracts/
sudo rsync -a --delete \$S/opt/config/ /opt/caravel/config/
sudo rsync -a --delete \$S/opt/relayer/ /opt/caravel/relayer/
sudo mkdir -p /opt/caravel/relayer-feeds
sudo rsync -a --delete \$S/opt/relayer-feeds/ /opt/caravel/relayer-feeds/
sudo rsync -a --delete \$S/opt/web/ /opt/caravel/web/
sudo chown -R caravel:caravel /opt/caravel/bin /opt/caravel/contracts /opt/caravel/config /opt/caravel/relayer /opt/caravel/relayer-feeds /opt/caravel/web
sudo chmod 755 /opt/caravel/bin/caravel-perps-node
(cd /opt/caravel/relayer && sudo -u caravel env HOME=/opt/caravel npm ci --omit=dev --silent --no-audit --no-fund)
(cd /opt/caravel/relayer-feeds/perps && sudo -u caravel env HOME=/opt/caravel npm ci --omit=dev --silent --no-audit --no-fund)
sudo cp \$S/systemd/* /etc/systemd/system/
sudo cp \$S/Caddyfile /etc/caddy/Caddyfile
rm -rf \$S
sudo systemctl daemon-reload
sudo systemctl enable --now caravel-sequencer > /dev/null 2>&1
for i in 1 2 3; do sudo systemctl enable --now caravel-validator@\$i > /dev/null 2>&1; done
sudo systemctl enable --now caravel-relayer > /dev/null 2>&1
sudo systemctl restart caravel-sequencer caravel-validator@1 caravel-validator@2 caravel-validator@3 caravel-relayer
sudo systemctl reload caddy || sudo systemctl restart caddy
sleep 5
systemctl is-active caravel-sequencer caravel-validator@1 caravel-validator@2 caravel-validator@3 caravel-relayer caddy | paste -sd' ' -"
echo "== https://$HOST/v1/status"
