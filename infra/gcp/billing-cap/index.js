// Billing cap for the Caravel testnet project (Google Cloud): when a budget
// notification reports cost above the budget, remove the project's billing
// account, which stops its resources. Budgets alone only alert.
// Follows docs.cloud.google.com/billing/docs/how-to/disable-billing-with-notifications
// (checked 2026-09-29) with no client library: a metadata-server token and one
// Cloud Billing API call.
const functions = require("@google-cloud/functions-framework");

const PROJECT_ID = process.env.PROJECT_ID;
// DRY_RUN=1 logs what it would do without touching billing.
const DRY_RUN = process.env.DRY_RUN === "1";

async function token() {
  const r = await fetch("http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token", {
    headers: { "Metadata-Flavor": "Google" },
  });
  if (!r.ok) throw new Error(`metadata token: HTTP ${r.status}`);
  return (await r.json()).access_token;
}

async function billing(method, body) {
  const r = await fetch(`https://cloudbilling.googleapis.com/v1/projects/${PROJECT_ID}/billingInfo`, {
    method,
    headers: { authorization: `Bearer ${await token()}`, "content-type": "application/json" },
    body: body ? JSON.stringify(body) : undefined,
  });
  const text = await r.text();
  if (!r.ok) throw new Error(`billingInfo ${method}: HTTP ${r.status} ${text}`);
  return JSON.parse(text);
}

functions.cloudEvent("stopBilling", async (event) => {
  if (!PROJECT_ID) throw new Error("PROJECT_ID is not set");
  const data = JSON.parse(Buffer.from(event.data.message.data, "base64").toString());
  const cost = Number(data.costAmount);
  const budget = Number(data.budgetAmount);
  console.log(JSON.stringify({ budget: data.budgetDisplayName, cost, budget_amount: budget, currency: data.currencyCode }));
  if (!(cost > budget)) return;
  const info = await billing("GET");
  if (!info.billingEnabled) {
    console.log("billing is already disabled");
    return;
  }
  if (DRY_RUN) {
    console.log(`DRY_RUN: would disable billing on ${PROJECT_ID}`);
    return;
  }
  await billing("PUT", { billingAccountName: "" });
  console.log(`billing disabled on ${PROJECT_ID}: cost ${cost} > budget ${budget}`);
});
