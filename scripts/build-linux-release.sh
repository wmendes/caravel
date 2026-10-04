#!/usr/bin/env bash
# Assembles a Linux release from this checkout on any machine with Docker
# (M0.8, D-08): the Rust binaries are built in the pinned Rust image
# (versions.json images.rust_builder) for this Docker's architecture, and
# the rest (contracts, relayer, feeds, web app) is this checkout's own
# builds, which don't depend on the platform. Then build-images.sh can
# package it, on a Mac too.
#
#   scripts/build-linux-release.sh <out dir> [--templates "perps payments"]
#
# Build first, as for assemble-release.sh: ./scripts/build-contracts.sh and
# the npm builds. The binaries land in target/linux-<arch>/release, and
# cargo's cache in target/linux-<arch>/cargo-home, so a second run is quick.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT=""
TEMPLATES=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --templates) TEMPLATES="$2"; shift 2 ;;
    -h|--help) sed -n '2,14p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    -*) echo "build-linux-release: unknown option $1" >&2; exit 2 ;;
    *) OUT="$1"; shift ;;
  esac
done
[[ -n "$OUT" ]] || { echo "usage: scripts/build-linux-release.sh <out dir> [--templates \"perps payments\"]" >&2; exit 2; }
if [[ -z "$TEMPLATES" ]]; then
  TEMPLATES="$(cd "$ROOT/lanes" && for d in */; do [[ -d "$d/node" ]] && printf '%s ' "${d%/}"; done)"
fi

IMAGE="$(sed -n 's/^ *"rust_builder": *"\([^"]*\)".*/\1/p' "$ROOT/versions.json")"
TOOLCHAIN="$(sed -n 's/^ *"rust_toolchain": *"\([^"]*\)".*/\1/p' "$ROOT/versions.json")"
[[ -n "$IMAGE" && -n "$TOOLCHAIN" ]] || { echo "build-linux-release: versions.json lacks images.rust_builder or rust_toolchain" >&2; exit 1; }
ARCH="$(docker version -f '{{.Server.Arch}}')"
TARGET="target/linux-$ARCH"

packages=(-p caravel-cli)
for t in $TEMPLATES; do packages+=(-p "caravel-$t-node"); done
echo "build-linux-release: cargo build ${packages[*]} in $IMAGE (linux/$ARCH)"
# As this user, so the files are ours; RUSTUP_TOOLCHAIN keeps rustup from
# installing rust-toolchain.toml's Wasm target into the image.
docker run --rm --platform "linux/$ARCH" -u "$(id -u):$(id -g)" \
  -v "$ROOT:/src" -w /src \
  -e CARGO_HOME="/src/$TARGET/cargo-home" -e CARGO_TARGET_DIR="/src/$TARGET" \
  -e RUSTUP_TOOLCHAIN="$TOOLCHAIN" -e CARGO_TERM_COLOR=never \
  ${CARAVEL_COMMIT:+-e CARAVEL_COMMIT="$CARAVEL_COMMIT"} \
  "$IMAGE" cargo build --release --locked "${packages[@]}"

CARAVEL_BIN_DIR="$ROOT/$TARGET/release" "$ROOT/scripts/assemble-release.sh" "$OUT" --templates "$TEMPLATES"
