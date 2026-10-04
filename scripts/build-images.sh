#!/usr/bin/env bash
# Builds a release's container images (M0.8, D-01, DEC-111) and writes the
# release's IMAGES file, which `caravel apply` reads for the docker runtime.
#
#   scripts/build-images.sh <release-dir>                 # local images for this machine
#   scripts/build-images.sh <release-dir> --push --registry ghcr.io/wmendes
#   scripts/build-images.sh amd64=<dir> arm64=<dir> --push --registry ghcr.io/wmendes
#
# A release holds one platform's binaries, so a multi-arch image is built
# from one release per architecture (arch=dir), each pushed by digest, then
# joined into one manifest list. IMAGES is written into every release given.
#
# The images package what the release already holds; nothing is recompiled:
#   caravel-<template>-node   the node binary and engine Wasm (docker/node.Dockerfile)
#   caravel-relayer           the relayer and the feed modules (docker/relayer.Dockerfile)
#   caravel-<template>-web    Caddy and the template's web app (docker/web.Dockerfile)
# IMAGES has one "<role> <ref>" line per image, plus the pinned Caddy image
# for lanes without a web app. A pushed ref is a digest; a local one a tag.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
RELS=()
PUSH=0
REGISTRY=""
TAG=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --push) PUSH=1 ;;
    --registry) REGISTRY="${2%/}/"; shift ;;
    --tag) TAG="$2"; shift ;;
    -*) echo "build-images: unknown option $1" >&2; exit 2 ;;
    *=*) RELS+=("$1") ;;
    *) RELS+=("$(uname -m | sed 's/x86_64/amd64/; s/aarch64/arm64/')=$1") ;;
  esac
  shift
done
[[ ${#RELS[@]} -gt 0 ]] || { echo "usage: build-images.sh <release-dir>|<arch>=<dir>... [--push --registry R] [--tag T]" >&2; exit 2; }
if [[ ${#RELS[@]} -gt 1 && $PUSH == 0 ]]; then echo "build-images: several architectures need --push (a local store holds one)" >&2; exit 2; fi
if [[ $PUSH == 1 && -z "$REGISTRY" ]]; then echo "build-images: --push needs --registry" >&2; exit 2; fi
REL="${RELS[0]#*=}"
for r in "${RELS[@]}"; do
  [[ -f "${r#*=}/COMMIT" ]] || { echo "build-images: ${r#*=} is not an assembled release (no COMMIT)" >&2; exit 1; }
  [[ "$(cat "${r#*=}/COMMIT")" == "$(cat "$REL/COMMIT")" ]] || { echo "build-images: the releases are of different commits" >&2; exit 1; }
done
COMMIT="$(cat "$REL/COMMIT")"
if [[ -z "$TAG" ]]; then
  case "$COMMIT" in local-*) TAG="$COMMIT" ;; *) TAG="sha-${COMMIT:0:12}" ;; esac
fi

LINES=()
# build <role> <name> <dockerfile> [build args...]
build() {
  local role="$1" name="$2" file="$3"
  shift 3
  local ref="${REGISTRY}${name}:${TAG}"
  local args=(--file "$ROOT/docker/$file" --build-arg "COMMIT=$COMMIT")
  for a in "$@"; do args+=(--build-arg "$a"); done
  if [[ $PUSH == 0 ]]; then
    docker build "${args[@]}" --tag "$ref" "$REL" >&2
    LINES+=("$role $ref")
  else
    local digests=() r arch dir meta
    for r in "${RELS[@]}"; do
      arch="${r%%=*}"
      dir="${r#*=}"
      meta="$(mktemp)"
      docker buildx build "${args[@]}" --platform "linux/$arch" --tag "$ref-$arch" --provenance=false --push --metadata-file "$meta" "$dir" >&2
      digests+=("${REGISTRY}${name}@$(sed -n 's/.*"containerimage.digest": *"\([^"]*\)".*/\1/p' "$meta")")
      rm -f "$meta"
    done
    docker buildx imagetools create --tag "$ref" "${digests[@]}" >&2
    LINES+=("$role ${REGISTRY}${name}@$(docker buildx imagetools inspect "$ref" --format '{{.Manifest.Digest}}')")
  fi
  echo "build-images: $role ${LINES[${#LINES[@]}-1]#* }" >&2
}

for bin in "$REL"/bin/caravel-*-node; do
  [[ -e "$bin" ]] || continue
  t="$(basename "$bin")"
  t="${t#caravel-}"
  t="${t%-node}"
  build "$t-node" "caravel-$t-node" node.Dockerfile "TEMPLATE=$t"
  if [[ -d "$REL/web/$t" ]]; then build "$t-web" "caravel-$t-web" web.Dockerfile "TEMPLATE=$t"; fi
done
if [[ -d "$REL/relayer" ]]; then
  for r in "${RELS[@]}"; do mkdir -p "${r#*=}/relayer-feeds"; done
  build relayer caravel-relayer relayer.Dockerfile
fi
# The Caddy image the web images are built on, for a lane that serves no web app.
CADDY="$(sed -n 's/^FROM \(caddy:[^ ]*\).*/\1/p' "$ROOT/docker/web.Dockerfile")"
LINES+=("caddy docker.io/library/$CADDY")

for r in "${RELS[@]}"; do printf '%s\n' "${LINES[@]}" > "${r#*=}/IMAGES"; done
echo "build-images: wrote IMAGES ($COMMIT)" >&2
printf '%s\n' "${LINES[@]}"
