#!/usr/bin/env bash
# Checks a CI release against the live testnet lane before deploy-vm.sh
# installs it (M0.5 P-07, RUNBOOK "Upgrading the VM"). Nothing live changes:
#
#   1. stage: the release goes to /opt/caravel/staging/<commit>, beside the
#      live install;
#   2. check-store: the release's binary re-executes a copy of the live
#      sequencer database through the engine Wasm, with the release's lane file;
#   3. shadow: the release's validator follows the live sequencer from block 1
#      as the transient unit caravel-shadow, with a throwaway key that is in no
#      signer set, so nobody asks it to sign;
#   4. compare: once it has caught up, every checkpoint header it computed must
#      equal the sequencer's.
#
#   gh run download <run id> -n caravel-release-linux-x86_64 -D /tmp/release
#   RELEASE_DIR=/tmp/release ./scripts/vm-preflight.sh             # 1-3, then 4 once caught up
#   RELEASE_DIR=/tmp/release STEP=compare ./scripts/vm-preflight.sh  # 4 again, later
#   STEP=stop ./scripts/vm-preflight.sh                              # stop the shadow, keep its store
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
PROJECT="${PROJECT:-caravel-testnet}"
ZONE="${ZONE:-us-central1-a}"
VM="${VM:-caravel-1}"
STEP="${STEP:-all}"
G=(--zone "$ZONE" --project "$PROJECT" --tunnel-through-iap --quiet)
vm() { gcloud compute ssh "$VM" "${G[@]}" --command "$1" 2> >(grep -v -e NumPy -e 'increasing_the_tcp' -e '^WARNING: *$' -e '^$' >&2); }
fail() { echo "FAIL: $*" >&2; exit 1; }

if [[ "$STEP" == "stop" ]]; then
  vm "sudo systemctl stop caravel-shadow 2>/dev/null; systemctl is-active caravel-shadow || true"
  exit 0
fi

RELEASE_DIR="${RELEASE_DIR:?set RELEASE_DIR to the CI release artifact}"
COMMIT="$(cut -c1-12 "$RELEASE_DIR/COMMIT")"
S="/opt/caravel/staging/$COMMIT"
echo "== release $COMMIT"
(cd "$RELEASE_DIR" && (sha256sum -c SHA256SUMS 2>/dev/null || shasum -a 256 -c SHA256SUMS)) > /dev/null || fail "SHA256SUMS"
[[ -x "$RELEASE_DIR/bin/caravel-perps-node" ]] || fail "the release has no bin/caravel-perps-node"

if [[ "$STEP" == "all" ]]; then
  echo "== stage to $S"
  T="$(mktemp -d)"
  trap 'rm -rf "$T"' EXIT
  mkdir -p "$T/bin" "$T/config"
  cp "$RELEASE_DIR/bin/caravel-perps-node" "$T/bin/"
  cp lanes/perps/config/lane.caravel-perps.testnet.toml "$T/config/"
  # check-store reads the live sequencer config with the copy of its database.
  sed -e "s#^lane = .*#lane = \"$S/config/lane.caravel-perps.testnet.toml\"#" \
      -e "s#^db = .*#db = \"$S/data/sequencer-copy.sqlite\"#" \
      lanes/perps/deploy/testnet/sequencer.toml > "$T/config/sequencer.toml"
  sed -e "s#^listen = .*#listen = \"127.0.0.1:8094\"#" \
      -e "s#^key_file = .*#key_file = \"$S/keys/shadow.key\"#" \
      -e "s#^lane = .*#lane = \"$S/config/lane.caravel-perps.testnet.toml\"#" \
      -e "s#^db = .*#db = \"$S/data/shadow.sqlite\"#" \
      lanes/perps/deploy/testnet/validator-1.toml > "$T/config/shadow.toml"
  COPYFILE_DISABLE=1 tar --no-xattrs -czf "$T.tgz" -C "$T" .
  gcloud compute scp "${G[@]}" "$T.tgz" "$VM:/tmp/caravel-staging.tgz" 2> /dev/null
  rm -f "$T.tgz"
  vm "set -e
sudo mkdir -p $S/data $S/keys
sudo tar -xzf /tmp/caravel-staging.tgz -C $S && rm -f /tmp/caravel-staging.tgz
sudo chown -R caravel:caravel /opt/caravel/staging
sudo chmod 755 $S $S/bin $S/config
sudo chmod 700 $S/keys
sudo -u caravel sh -c 'node -e \"console.log(require(\\\"/opt/caravel/relayer/node_modules/@stellar/stellar-sdk\\\").Keypair.random().secret())\" > $S/keys/shadow.key && chmod 600 $S/keys/shadow.key'
sudo -u caravel $S/bin/caravel-perps-node --version"

  echo "== check-store on a copy of the live sequencer database"
  vm "set -e
sudo -u caravel python3 -c 'import sqlite3; s = sqlite3.connect(\"/opt/caravel/data/sequencer.sqlite\"); d = sqlite3.connect(\"$S/data/sequencer-copy.sqlite\"); s.backup(d); d.close(); s.close()'
sudo -u caravel env HOME=/opt/caravel $S/bin/caravel-perps-node check-store --config $S/config/sequencer.toml"

  echo "== shadow validator (caravel-shadow, 127.0.0.1:8094)"
  vm "set -e
sudo systemctl stop caravel-shadow 2>/dev/null || true
sudo systemctl reset-failed caravel-shadow 2>/dev/null || true
sudo systemd-run --unit caravel-shadow --uid caravel --gid caravel --setenv RUST_LOG=info \
  --property MemoryMax=400M --property Nice=10 \
  $S/bin/caravel-perps-node validator --config $S/config/shadow.toml
sleep 5; systemctl is-active caravel-shadow"
fi

echo "== compare the shadow's checkpoint headers with the sequencer's"
vm "python3 - <<'PY'
import json, urllib.request
get = lambda u: json.load(urllib.request.urlopen(u, timeout=10))
seq, sh = get('http://127.0.0.1:8080/v1/status'), get('http://127.0.0.1:8094/v1/status')
print('sequencer height', seq['height'], 'shadow height', sh['height'], 'shadow halted', sh['halted'], 'template', sh.get('template'))
last = int(sh['checkpoints']['computed'] or 0)
bad = [s for s in range(1, last + 1)
       if get(f'http://127.0.0.1:8094/v1/checkpoints/{s}')['header_hash'] != get(f'http://127.0.0.1:8080/v1/checkpoints/{s}')['header_hash']]
print('checkpoints compared', last, 'differing', bad[:10])
if bad or sh['halted'] or sh['suspicious_blocks']:
    raise SystemExit('SHADOW MISMATCH')
PY"
