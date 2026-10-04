#!/usr/bin/env bash
# Once per project, before `tofu init`: turns on the two APIs the google
# provider needs to read a project (Resource Manager, Service Usage; both
# free) and makes the bucket that holds the env's state. The env then imports
# the bucket and manages it (versioned, private, prevent_destroy).
#
#   PROJECT=caravel-testnet BUCKET=caravel-testnet-tofu-state ./infra/opentofu/bootstrap-state.sh
set -euo pipefail
: "${PROJECT:?}" "${BUCKET:?}" "${LOCATION:=US-CENTRAL1}"
gcloud services enable cloudresourcemanager.googleapis.com serviceusage.googleapis.com storage.googleapis.com --project="$PROJECT"
if gcloud storage buckets describe "gs://$BUCKET" --project="$PROJECT" > /dev/null 2>&1; then
  echo "gs://$BUCKET exists"; exit 0
fi
gcloud storage buckets create "gs://$BUCKET" --project="$PROJECT" --location="$LOCATION" \
  --default-storage-class=STANDARD --uniform-bucket-level-access --public-access-prevention
gcloud storage buckets update "gs://$BUCKET" --versioning
echo "made gs://$BUCKET; now: tofu -chdir=infra/opentofu/envs/<env> init"
