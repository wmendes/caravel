#!/usr/bin/env bash
# Installs Caravel: a prebuilt release (M0.7, H-02, H-03), or built from a
# checkout (M0.6, DEC-076).
#
#   curl -fsSL https://raw.githubusercontent.com/wmendes/caravel/main/scripts/install.sh | bash
#
# downloads the latest release for this machine (x86_64 or arm64 Linux,
# arm64 macOS), checks it against the release's SHA256SUMS, and installs it.
# The Stellar CLI it needs comes too, pinned and checked, unless the one on
# PATH is already that version.
#
#   $PREFIX/bin/caravel, $PREFIX/bin/caravel-<template>-node
#   $PREFIX/bin/stellar-caravel -> caravel        (a Stellar CLI plugin)
#   $PREFIX/share/caravel/<release>/              (what `caravel apply` installs on hosts)
#   $PREFIX/share/caravel/current -> <release>
#
#   install.sh [--version vX.Y.Z] [--prefix DIR] [--no-stellar-cli] [--no-modify-path]
#   install.sh --archive caravel-<version>-<target>.tar.gz [--prefix DIR]
#   ./scripts/install.sh [--prefix DIR] [--templates "perps payments"] [--with-web]
#                        [--wasm-dir DIR] [--skip-build]       (in a checkout: build it)
#   ./scripts/install.sh --release [--version vX.Y.Z]          (in a checkout: download)
#
# PREFIX defaults to $CARAVEL_HOME, else ~/.caravel. --wasm-dir takes the
# contracts from the CI contracts-wasm artifact, the builds of record
# (needed for testnet lanes when this machine isn't x86_64 Linux, DEC-033).
# --with-web also builds each template's web app.
#
# PATH: unless $PREFIX/bin is already on it, the installer writes
# $PREFIX/env (puts $PREFIX/bin first on PATH, once) and sources it from your
# shell's startup files, the way rustup does: ~/.profile, ~/.bashrc and
# ~/.bash_profile when they exist, zsh's .zshenv, and fish's conf.d. New
# shells find caravel; this one needs `. "$PREFIX/env"`. --no-modify-path
# (or CARAVEL_NO_MODIFY_PATH=1) leaves every file outside PREFIX alone.
set -euo pipefail

# The checkout this script is in, if any: piped from curl, it has none.
ROOT=""
SRC="${BASH_SOURCE[0]:-}"
if [[ -n "$SRC" && -f "$SRC" && -f "$(dirname "$SRC")/assemble-release.sh" ]]; then
  ROOT="$(cd "$(dirname "$SRC")/.." && pwd)"
fi
REPO="${CARAVEL_REPO:-wmendes/caravel}"
# Must match versions.json "stellar_cli" (scripts/check-versions.mjs checks it).
STELLAR_CLI_VERSION="28.1.0"
PREFIX="${CARAVEL_HOME:-$HOME/.caravel}"
MODIFY_PATH=1
[[ "${CARAVEL_NO_MODIFY_PATH:-0}" == 1 ]] && MODIFY_PATH=0
TEMPLATES=""
WITH_WEB=0
WASM_DIR=""
SKIP_BUILD=0
ARCHIVE=""
VERSION=""
RELEASE=0
STELLAR_CLI=1
while [[ $# -gt 0 ]]; do
  case "$1" in
    --prefix) PREFIX="$2"; shift 2 ;;
    --templates) TEMPLATES="$2"; shift 2 ;;
    --with-web) WITH_WEB=1; shift ;;
    --wasm-dir) WASM_DIR="$2"; shift 2 ;;
    --skip-build) SKIP_BUILD=1; shift ;;
    --archive) ARCHIVE="$2"; shift 2 ;;
    --version) VERSION="$2"; shift 2 ;;
    --release) RELEASE=1; shift ;;
    --no-stellar-cli) STELLAR_CLI=0; shift ;;
    --no-modify-path) MODIFY_PATH=0; shift ;;
    -h|--help) sed -n '2,29p' "${SRC:-$0}" 2> /dev/null | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "install: unknown option $1 (see --help)" >&2; exit 2 ;;
  esac
done
log() { printf '\n== %s\n' "$*"; }
fail() { echo "install: $*" >&2; exit 1; }
if command -v sha256sum > /dev/null; then SHA256=(sha256sum); else SHA256=(shasum -a 256); fi

# Moves a release directory into PREFIX and links its binaries.
install_release() {
  local dir="$1" name
  name="$(cut -c1-12 < "$dir/COMMIT")"
  log "install into $PREFIX"
  mkdir -p "$PREFIX/bin" "$PREFIX/share/caravel"
  rm -rf "$PREFIX/share/caravel/$name"
  mv "$dir" "$PREFIX/share/caravel/$name"
  ln -sfn "$name" "$PREFIX/share/caravel/current"
  for b in "$PREFIX/share/caravel/$name"/bin/*; do
    install -m 755 "$b" "$PREFIX/bin/"
  done
  ln -sfn caravel "$PREFIX/bin/stellar-caravel"
  "$PREFIX/bin/caravel" version
  setup_path
}

# Puts $PREFIX/bin on PATH for new shells (see the header). Safe to run again:
# each startup file gets one line, and env adds the directory once.
ON_PATH=1
setup_path() {
  case ":$PATH:" in *":$PREFIX/bin:"*) return ;; esac
  ON_PATH=0
  if [[ "$MODIFY_PATH" != 1 ]]; then
    printf '\nAdd Caravel to your PATH:\n  export PATH="%s/bin:$PATH"\n' "$PREFIX"
    return
  fi
  cat > "$PREFIX/env" <<ENV
# Caravel's bin directory first on PATH, once (written by install.sh).
case ":\${PATH}:" in
  *":$PREFIX/bin:"*) ;;
  *) export PATH="$PREFIX/bin:\$PATH" ;;
esac
ENV
  local line=". \"$PREFIX/env\"" changed=() f
  local files=("$HOME/.profile")
  for f in "$HOME/.bashrc" "$HOME/.bash_profile"; do [[ -f "$f" ]] && files+=("$f"); done
  if [[ -n "${ZDOTDIR:-}" || "${SHELL:-}" == */zsh || -f "$HOME/.zshrc" ]]; then files+=("${ZDOTDIR:-$HOME}/.zshenv"); fi
  for f in "${files[@]}"; do
    if ! grep -qsF "$line" "$f"; then
      printf '\n# Caravel\n%s\n' "$line" >> "$f"
      changed+=("$f")
    fi
  done
  if [[ "${SHELL:-}" == */fish || -d "$HOME/.config/fish" ]]; then
    f="$HOME/.config/fish/conf.d/caravel.fish"
    mkdir -p "$(dirname "$f")"
    printf '# Caravel (written by install.sh)\nfish_add_path --prepend %s\n' "$PREFIX/bin" > "$f"
    changed+=("$f")
  fi
  if (( ${#changed[@]} )); then
    printf '\nAdded %s/bin to your PATH in: %s\n' "$PREFIX" "${changed[*]/#$HOME/~}"
  fi
}

# The pinned Stellar CLI next to caravel, checked against GitHub's published
# digests (docs/SOURCES.md), unless the one on PATH is that version.
stellar_cli() {
  if command -v stellar > /dev/null && [[ "$(stellar --version | head -1 | awk '{print $2}')" == "$STELLAR_CLI_VERSION" ]]; then
    return 0
  fi
  local asset sum tmp
  case "$(uname -s)-$(uname -m)" in
    Linux-x86_64) asset=x86_64-unknown-linux-gnu; sum=c1680deee94301d33ada7a17f98411e642a4248c727afbd2e43050d345746462 ;;
    Linux-aarch64|Linux-arm64) asset=aarch64-unknown-linux-gnu; sum=b1e6ab53b0dd673400110fbd665a525cd0a969449943a2990a782f20cc7abe98 ;;
    Darwin-arm64) asset=aarch64-apple-darwin; sum=accddc9a44dc51e99e5fc6d976fb87a327f8b5cd71bc72c0dab3ac685dfbc7ec ;;
    Darwin-x86_64) asset=x86_64-apple-darwin; sum=edae3f5c8380c75110a08dedffe599ef2fc7555ff4cbf37576193e26a9e4589f ;;
    *) echo "install: note: install the Stellar CLI $STELLAR_CLI_VERSION yourself (cargo install --locked stellar-cli@$STELLAR_CLI_VERSION)" >&2; return 0 ;;
  esac
  log "Stellar CLI $STELLAR_CLI_VERSION"
  tmp="$(mktemp -d)"
  curl -fsSL -o "$tmp/stellar.tar.gz" "https://github.com/stellar/stellar-cli/releases/download/v$STELLAR_CLI_VERSION/stellar-cli-$STELLAR_CLI_VERSION-$asset.tar.gz" \
    || fail "downloading the Stellar CLI failed"
  [[ "$("${SHA256[@]}" "$tmp/stellar.tar.gz" | awk '{print $1}')" == "$sum" ]] || fail "the Stellar CLI download does not match its published sha256"
  tar -xzf "$tmp/stellar.tar.gz" -C "$tmp" stellar
  mkdir -p "$PREFIX/bin"
  install -m 755 "$tmp/stellar" "$PREFIX/bin/stellar"
  rm -rf "$tmp"
  if command -v stellar > /dev/null && [[ "$(command -v stellar)" != "$PREFIX/bin/stellar" ]]; then
    echo "install: note: $PREFIX/bin/stellar ($STELLAR_CLI_VERSION) comes first once $PREFIX/bin is first on PATH" >&2
  fi
}

# What to run now, and in this shell first if PATH changed only for new ones.
next_steps() {
  if [[ "$ON_PATH" == 0 && "$MODIFY_PATH" == 1 ]]; then
    printf '\nOpen a new terminal, or run this to use caravel in this one:\n  . "%s/env"\n' "$PREFIX"
  fi
  printf '\nNext: caravel init payments my-lane && cd my-lane && caravel apply\n'
}

# A release from GitHub for this machine: its archive and the release's
# SHA256SUMS, then installed as an archive.
if [[ -z "$ARCHIVE" && ( -z "$ROOT" || "$RELEASE" == 1 ) ]]; then
  command -v curl > /dev/null || fail "curl is not installed"
  case "$(uname -s)-$(uname -m)" in
    Linux-x86_64) TARGET=x86_64-linux ;;
    Linux-aarch64|Linux-arm64) TARGET=aarch64-linux ;;
    Darwin-arm64) TARGET=aarch64-macos ;;
    *) fail "there is no prebuilt Caravel for $(uname -sm) yet: build it from a checkout (git clone https://github.com/$REPO && cd caravel && ./scripts/install.sh)" ;;
  esac
  if [[ -z "$VERSION" ]]; then
    VERSION="$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -1)" || true
    [[ -n "$VERSION" ]] || fail "no published Caravel release found at github.com/$REPO (build from a checkout instead)"
  fi
  NAME="caravel-${VERSION#v}-$TARGET.tar.gz"
  # CARAVEL_DOWNLOAD_BASE: a mirror laid out like GitHub's (tests use it).
  BASE="${CARAVEL_DOWNLOAD_BASE:-https://github.com/$REPO/releases/download}/$VERSION"
  DL="$(mktemp -d)"
  trap 'rm -rf "$DL"' EXIT
  log "Caravel $VERSION for $TARGET"
  curl -fsSL -o "$DL/$NAME" "$BASE/$NAME" || fail "downloading $BASE/$NAME failed"
  curl -fsSL -o "$DL/SHA256SUMS" "$BASE/SHA256SUMS" || fail "downloading $BASE/SHA256SUMS failed"
  grep "  $NAME\$" "$DL/SHA256SUMS" > "$DL/$NAME.sha256" || fail "$NAME is not in the release's SHA256SUMS"
  # The release's container images by digest (D-01): releases before M0.8 have none.
  curl -fsSL -o "$DL/IMAGES" "$BASE/IMAGES" 2> /dev/null || rm -f "$DL/IMAGES"
  ARCHIVE="$DL/$NAME"
fi

# A prebuilt archive: checked against its .sha256 when there is one, then
# its own SHA256SUMS, then installed. No build tools needed.
if [[ -n "$ARCHIVE" ]]; then
  [[ -f "$ARCHIVE" ]] || fail "no archive $ARCHIVE"
  if [[ -f "$ARCHIVE.sha256" ]]; then
    want="$(awk '{print $1}' "$ARCHIVE.sha256")"
    have="$("${SHA256[@]}" "$ARCHIVE" | awk '{print $1}')"
    [[ "$want" == "$have" ]] || fail "$ARCHIVE does not match its .sha256"
  fi
  mkdir -p "$PREFIX/share/caravel"
  STAGE="$PREFIX/share/caravel/.stage-$$"
  rm -rf "$STAGE"
  mkdir -p "$STAGE"
  trap 'rm -rf "$STAGE" ${DL:+"$DL"}' EXIT
  tar -xzf "$ARCHIVE" -C "$STAGE"
  dir="$(find "$STAGE" -mindepth 1 -maxdepth 1 -type d | head -1)"
  [[ -f "$dir/COMMIT" && -f "$dir/SHA256SUMS" ]] || fail "$ARCHIVE is not a Caravel release"
  (cd "$dir" && "${SHA256[@]}" -c --quiet SHA256SUMS) || fail "$ARCHIVE: a file does not match SHA256SUMS"
  if [[ -n "${DL:-}" && -f "$DL/IMAGES" ]]; then cp "$DL/IMAGES" "$dir/IMAGES"; fi
  [[ "$STELLAR_CLI" == 1 ]] && stellar_cli
  install_release "$dir"
  next_steps
  if [[ -f "$PREFIX/share/caravel/current/IMAGES" ]]; then
    printf 'A lane on your machine needs Docker with Compose: its nodes run in containers. caravel doctor checks.\n'
  else
    printf 'A lane on your machine also needs Docker (the local Stellar network) and Node.js 22 (its relayer); caravel doctor checks.\n'
  fi
  exit 0
fi

[[ -n "$ROOT" ]] || fail "building from source needs a checkout: git clone https://github.com/$REPO"
cd "$ROOT"
if [[ -z "$TEMPLATES" ]]; then
  TEMPLATES="$(cd lanes && for d in */; do [[ -d "$d/node" ]] && printf '%s ' "${d%/}"; done)"
fi
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
  # The release's directories that exist (a payments-only one has no
  # relayer-feeds): find fails on a missing one, and this runs under pipefail.
  (cd "$STAGE/release" && dirs=() && for d in bin contracts relayer relayer-feeds; do if [[ -d "$d" ]]; then dirs+=("$d"); fi; done \
    && find "${dirs[@]}" -type f -not -path '*/node_modules/*' | LC_ALL=C sort | xargs "${SHA256[@]}" > SHA256SUMS)
  if [[ -z "$COMMIT" ]]; then
    echo "local-$("${SHA256[@]}" "$STAGE/release/SHA256SUMS" | cut -c1-8)" > "$STAGE/release/COMMIT"
  fi
fi
install_release "$STAGE/release"
next_steps
