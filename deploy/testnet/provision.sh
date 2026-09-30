#!/usr/bin/env bash
# One-time setup of the testnet VM (Ubuntu 24.04, run as root). Installs
# Caddy (its official apt repository, caddyserver.com/docs/install) and
# Node.js from the official tarball checked against SHASUMS256.txt, creates
# the unprivileged `caravel` user and /opt/caravel, and adds 1 GB of swap.
set -euo pipefail
NODE_VERSION="${NODE_VERSION:-v22.23.3}"
export DEBIAN_FRONTEND=noninteractive

apt-get update -q
apt-get install -y -q debian-keyring debian-archive-keyring apt-transport-https curl jq rsync sqlite3 xz-utils

if ! command -v caddy > /dev/null; then
  curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/gpg.key' | gpg --dearmor -o /usr/share/keyrings/caddy-stable-archive-keyring.gpg
  curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/debian.deb.txt' > /etc/apt/sources.list.d/caddy-stable.list
  chmod o+r /usr/share/keyrings/caddy-stable-archive-keyring.gpg /etc/apt/sources.list.d/caddy-stable.list
  apt-get update -q
  apt-get install -y -q caddy
fi

if [[ "$(node --version 2>/dev/null || true)" != "$NODE_VERSION" ]]; then
  tmp="$(mktemp -d)"
  f="node-$NODE_VERSION-linux-x64.tar.xz"
  curl -fsSL -o "$tmp/$f" "https://nodejs.org/dist/$NODE_VERSION/$f"
  curl -fsSL -o "$tmp/SHASUMS256.txt" "https://nodejs.org/dist/$NODE_VERSION/SHASUMS256.txt"
  (cd "$tmp" && grep " $f\$" SHASUMS256.txt | sha256sum -c -)
  tar -xJf "$tmp/$f" -C /usr/local --strip-components=1
  ln -sf /usr/local/bin/node /usr/bin/node
  rm -rf "$tmp"
fi

id caravel > /dev/null 2>&1 || useradd --system --home-dir /opt/caravel --shell /usr/sbin/nologin caravel
install -d -o caravel -g caravel -m 755 /opt/caravel /opt/caravel/bin /opt/caravel/contracts /opt/caravel/config /opt/caravel/relayer /opt/caravel/web
install -d -o caravel -g caravel -m 700 /opt/caravel/keys /opt/caravel/data

if [[ ! -f /swapfile ]]; then
  fallocate -l 1G /swapfile && chmod 600 /swapfile && mkswap /swapfile > /dev/null && swapon /swapfile
  echo '/swapfile none swap sw 0 0' >> /etc/fstab
fi
echo "provisioned: caddy $(caddy version | awk '{print $1}'), node $(node --version)"
