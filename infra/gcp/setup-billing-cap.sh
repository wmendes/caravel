#!/usr/bin/env bash
# The Caravel testnet project's spending limit on Google Cloud (DEC-045):
# a monthly budget that alerts at 50/90/100% and a function that removes the
# project's billing account at 100%, which stops everything in the project.
# Google's caveats apply: notifications lag real costs, so this is not an
# exact guarantee, and resources left without billing can be deleted.
#
#   PROJECT=caravel-testnet BILLING=019CC0-F55931-51DBCE LIMIT=100BRL ./infra/gcp/setup-billing-cap.sh
set -euo pipefail
: "${PROJECT:?}" "${BILLING:?}" "${LIMIT:=100BRL}"
REGION="${REGION:-us-central1}"
SA="billing-cap@$PROJECT.iam.gserviceaccount.com"
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

gcloud services enable billingbudgets.googleapis.com cloudbilling.googleapis.com pubsub.googleapis.com \
  cloudfunctions.googleapis.com run.googleapis.com cloudbuild.googleapis.com artifactregistry.googleapis.com \
  eventarc.googleapis.com logging.googleapis.com --project="$PROJECT"
gcloud pubsub topics create billing-cap --project="$PROJECT"
gcloud iam service-accounts create billing-cap --display-name="Caravel billing cap" --project="$PROJECT"
# Least privilege, on this project only: read the project, remove its billing.
for role in roles/billing.projectManager roles/browser; do
  gcloud projects add-iam-policy-binding "$PROJECT" --member="serviceAccount:$SA" --role="$role" --condition=None --quiet > /dev/null
done
gcloud billing budgets create --billing-account="$BILLING" --billing-project="$PROJECT" \
  --display-name="Caravel testnet monthly limit" --budget-amount="$LIMIT" --calendar-period=month \
  --filter-projects="projects/$PROJECT" --threshold-rule=percent=0.5 --threshold-rule=percent=0.9 \
  --threshold-rule=percent=1.0 --notifications-rule-pubsub-topic="projects/$PROJECT/topics/billing-cap"
gcloud functions deploy stop-billing --gen2 --project="$PROJECT" --region="$REGION" --runtime=nodejs22 \
  --source="$DIR/billing-cap" --entry-point=stopBilling --trigger-topic=billing-cap --service-account="$SA" \
  --set-env-vars="PROJECT_ID=$PROJECT" --max-instances=1 --memory=256Mi --quiet
gcloud run services update stop-billing --region="$REGION" --project="$PROJECT" --ingress=internal --quiet
gcloud run services add-iam-policy-binding stop-billing --region="$REGION" --project="$PROJECT" \
  --member="serviceAccount:$SA" --role=roles/run.invoker --quiet > /dev/null
echo "Test: gcloud pubsub topics publish billing-cap --project=$PROJECT --message='{\"costAmount\":1,\"budgetAmount\":100}' (a no-op; above the budget it disables billing)"
