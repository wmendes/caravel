#!/usr/bin/env bash
# A Caravel host's setup (Ubuntu 24.04, as root), run at every boot and safe
# to run again: Docker Engine and the Compose plugin from Docker's apt
# repository (docs.docker.com/engine/install/ubuntu), pinned to versions.json
# "docker", with the repository key checked by fingerprint; /opt/caravel for
# the containers' user (10001); 1 GB of swap. `caravel apply` does the rest.
set -euo pipefail
DOCKER_VERSION="5:29.8.2-1~ubuntu.24.04~noble"
COMPOSE_VERSION="5.6.0-1~ubuntu.24.04~noble"
CONTAINERD_VERSION="2.3.6-1~ubuntu.24.04~noble"
DOCKER_KEY_FPR="9DC858229FC7DD38854AE2D88D81803C0EBFCD88"
export DEBIAN_FRONTEND=noninteractive

have() { [[ "$(dpkg-query -W -f='${Version}' "$1" 2>/dev/null || true)" == "$2" ]]; }

if ! have docker-ce "$DOCKER_VERSION" || ! have docker-compose-plugin "$COMPOSE_VERSION"; then
  apt-get update -q
  apt-get install -y -q ca-certificates curl gnupg jq sqlite3
  install -d -m 755 /etc/apt/keyrings
  curl -fsSL https://download.docker.com/linux/ubuntu/gpg -o /tmp/docker.asc
  fpr="$(gpg --show-keys --with-colons /tmp/docker.asc | awk -F: '/^fpr/ { print $10; exit }')"
  [[ "$fpr" == "$DOCKER_KEY_FPR" ]] || { echo "unexpected Docker key $fpr" >&2; exit 1; }
  install -m 644 /tmp/docker.asc /etc/apt/keyrings/docker.asc
  . /etc/os-release
  echo "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/docker.asc] https://download.docker.com/linux/ubuntu $VERSION_CODENAME stable" \
    > /etc/apt/sources.list.d/docker.list
  apt-get update -q
  apt-get install -y -q --allow-downgrades "docker-ce=$DOCKER_VERSION" "docker-ce-cli=$DOCKER_VERSION" \
    "containerd.io=$CONTAINERD_VERSION" "docker-compose-plugin=$COMPOSE_VERSION"
  apt-mark hold docker-ce docker-ce-cli containerd.io docker-compose-plugin
fi
systemctl enable --now docker

install -d -o 10001 -g 10001 -m 755 /opt/caravel
for d in keys data; do install -d -o 10001 -g 10001 -m 700 "/opt/caravel/$d"; done

if [[ ! -f /swapfile ]]; then
  fallocate -l 1G /swapfile && chmod 600 /swapfile && mkswap /swapfile > /dev/null
  echo '/swapfile none swap sw 0 0' >> /etc/fstab
fi
swapon --show=NAME --noheadings | grep -qx /swapfile || swapon /swapfile
echo "caravel host: $(docker --version), $(docker compose version)"
