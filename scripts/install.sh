#!/usr/bin/env bash
# Builds Caravel from this checkout and installs it (M0.6, DEC-076):
#
#   $PREFIX/bin/caravel, $PREFIX/bin/caravel-<template>-node
#   $PREFIX/bin/stellar-caravel -> caravel        (a Stellar CLI plugin)
#   $PREFIX/share/caravel/<release>/              (what `caravel apply` installs on hosts)
#   $PREFIX/share/caravel/current -> <release>
#
#   ./scripts/install.sh [--prefix DIR] [--templates "perps payments"] [--with-web]
#                        [--wasm-dir DIR] [--skip-build]
#
# PREFIX defaults to $CARAVEL_HOME, else ~/.caravel. --wasm-dir takes the
# contracts from the CI contracts-wasm artifact, the builds of record
# (needed for testnet lanes when this machine isn't x86_64 Linux, DEC-033).
# --with-web also builds each template's web app. Nothing outside PREFIX is
# touched; add $PREFIX/bin to your PATH.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PREFIX="${CARAVEL_HOME:-$HOME/.caravel}"
TEMPLATES=""
WITH_WEB=0
WASM_DIR=""
SKIP_BUILD=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --prefix) PREFIX="$2"; shift 2 ;;
    --templates) TEMPLATES="$2"; shift 2 ;;
    --with-web) WITH_WEB=1; shift ;;
    --wasm-dir) WASM_DIR="$2"; shift 2 ;;
    --skip-build) SKIP_BUILD=1; shift ;;
    -h|--help) sed -n '2,17p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "install: unknown option $1 (see --help)" >&2; exit 2 ;;
  esac
done
cd "$ROOT"
if [[ -z "$TEMPLATES" ]]; then
  TEMPLATES="$(cd lanes && for d in */; do [[ -d "$d/node" ]] && printf '%s ' "${d%/}"; done)"
fi
log() { printf '\n== %s\n' "$*"; }
fail() { echo "install: $*" >&2; exit 1; }

log "tools"
command -v cargo > /dev/null || fail "cargo is not installed (https://rustup.rs)"
command -v node > /dev/null || fail "Node.js 22 or later is not installed"
[[ "$(node -p 'process.versions.node.split(".")[0]')" -ge 22 ]] || fail "Node.js $(node --version) is installed; Caravel needs 22 or later"
command -v npm > /dev/null || fail "npm is not installed"
want_cli="$(node -p 'require("./versions.json").stellar_cli')"
if ! command -v stellar > /dev/null; then
  fail "the Stellar CLI is not installed: cargo install --locked stellar-cli@$want_cli"
fi
have_cli="$(stellar --version | head -1 | awk '{print $2}')"
[[ "$have_cli" == "$want_cli" ]] || fail "stellar CLI $have_cli is installed; Caravel needs $want_cli: cargo install --locked stellar-cli@$want_cli"
echo "cargo $(cargo --version | awk '{print $2}'), node $(node --version), stellar $have_cli"

COMMIT=""
if git diff --quiet HEAD 2> /dev/null && [[ -z "$(git status --porcelain 2> /dev/null)" ]]; then
  COMMIT="$(git rev-parse HEAD)"
fi

if [[ "$SKIP_BUILD" == 0 ]]; then
  log "contracts"
  ./scripts/build-contracts.sh
  log "binaries"
  pkgs=(-p caravel-cli)
  for t in $TEMPLATES; do pkgs+=(-p "caravel-$t-node"); done
  # Each node reports the commit it was built from (`/v1/status`), which the
  # deploy tool compares with the release it installed.
  if [[ -n "$COMMIT" ]]; then
    CARAVEL_COMMIT="$COMMIT" cargo build --release --locked "${pkgs[@]}"
  else
    cargo build --release --locked "${pkgs[@]}"
  fi
  log "relayer"
  npm --prefix platform/relayer ci --no-audit --no-fund --loglevel=error
  npm --prefix platform/relayer run build
  for t in $TEMPLATES; do
    if [[ -d "lanes/$t/relayer-feeds" ]]; then
      npm --prefix "lanes/$t/relayer-feeds" ci --no-audit --no-fund --loglevel=error
      npm --prefix "lanes/$t/relayer-feeds" run build
    fi
    if [[ "$WITH_WEB" == 1 && -d "lanes/$t/web" ]]; then
      npm --prefix "lanes/$t/web" ci --no-audit --no-fund --loglevel=error
      npm --prefix "lanes/$t/web" run build
    fi
  done
fi

log "release"
# Staged inside the prefix, so the final move is a rename.
mkdir -p "$PREFIX/share/caravel"
STAGE="$PREFIX/share/caravel/.stage-$$"
rm -rf "$STAGE"
mkdir -p "$STAGE"
trap 'rm -rf "$STAGE"' EXIT
args=("$STAGE/release" --templates "$TEMPLATES")
[[ -n "$COMMIT" ]] && args+=(--commit "$COMMIT")
./scripts/assemble-release.sh "${args[@]}"
if [[ -n "$WASM_DIR" ]]; then
  [[ -f "$WASM_DIR/settlement.wasm" ]] || fail "$WASM_DIR has no settlement.wasm (the CI contracts-wasm artifact)"
  cp "$WASM_DIR"/*.wasm "$STAGE/release/contracts/"
  if command -v sha256sum > /dev/null; then SHA256=(sha256sum); else SHA256=(shasum -a 256); fi
  (cd "$STAGE/release" && find bin contracts relayer relayer-feeds -type f -not -path '*/node_modules/*' 2> /dev/null | LC_ALL=C sort | xargs "${SHA256[@]}" > SHA256SUMS)
  if [[ -z "$COMMIT" ]]; then
    echo "local-$("${SHA256[@]}" "$STAGE/release/SHA256SUMS" | cut -c1-8)" > "$STAGE/release/COMMIT"
  fi
fi
NAME="$(cut -c1-12 < "$STAGE/release/COMMIT")"

log "install into $PREFIX"
mkdir -p "$PREFIX/bin" "$PREFIX/share/caravel"
rm -rf "$PREFIX/share/caravel/$NAME"
mv "$STAGE/release" "$PREFIX/share/caravel/$NAME"
ln -sfn "$NAME" "$PREFIX/share/caravel/current"
for b in "$PREFIX/share/caravel/$NAME"/bin/*; do
  install -m 755 "$b" "$PREFIX/bin/"
done
ln -sfn caravel "$PREFIX/bin/stellar-caravel"

"$PREFIX/bin/caravel" version
case ":$PATH:" in
  *":$PREFIX/bin:"*) ;;
  *) printf '\nAdd Caravel to your PATH:\n  export PATH="%s/bin:$PATH"\n' "$PREFIX" ;;
esac
