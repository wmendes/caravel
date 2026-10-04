/**
 * "Get test USDC" (DEC-106): testnet XLM from friendbot when the account is
 * new, then one wallet signature for the USDC trustline and an XLM to USDC
 * path payment on the testnet DEX. What arrives is handed to the deposit form.
 */
import { useEffect, useState } from "react";

import { explain, stellar, type UsdcQuote } from "../api/stellar";
import { config } from "../config";
import { short, USDC } from "../format";

const AMOUNTS = [100n, 500n, 1000n];
/** XLM kept back for the account's reserves and fees. */
const XLM_RESERVE = 5n * USDC;

type Step = "idle" | "xlm" | "quote" | "sign" | "done";

export function FundPanel({ address, wallet, onFunded }: { address: string; wallet: bigint | null | undefined; onFunded: (usdc: bigint) => void }) {
  const [amount, setAmount] = useState(500n);
  const [xlm, setXlm] = useState<bigint | null | undefined>(undefined);
  const [quote, setQuote] = useState<UsdcQuote | null>(null);
  const [step, setStep] = useState<Step>("idle");
  const [err, setErr] = useState<string | null>(null);
  const [hash, setHash] = useState<string | null>(null);
  const enabled = !!config.horizonUrl;

  // The account's XLM, and a live quote for the chosen amount.
  useEffect(() => {
    if (!enabled) return;
    let live = true;
    void stellar
      .xlm(address)
      .then((v) => live && setXlm(v))
      .catch(() => live && setXlm(undefined));
    setQuote(null);
    void stellar
      .quoteUsdc(amount * USDC)
      .then((q) => live && setQuote(q))
      .catch((e) => live && setErr(e instanceof Error ? e.message : String(e)));
    return () => {
      live = false;
    };
  }, [address, amount, enabled, step === "done"]);

  if (!enabled) {
    return (
      <p className="hint">
        This deployment has no DEX to buy test USDC from. Get some from the{" "}
        <a href={config.faucetUrl} target="_blank" rel="noreferrer">
          Circle faucet
        </a>
        .
      </p>
    );
  }

  const needsXlm = xlm === null;
  const needsTrustline = wallet === null;
  const quotedXlm = quote ? BigInt(Math.ceil(Number(quote.xlm) * 1.02 * 1e7)) : null;
  const short_ = xlm != null && quotedXlm !== null && xlm < quotedXlm + XLM_RESERVE;
  const busy = step === "xlm" || step === "quote" || step === "sign";

  const run = async () => {
    setErr(null);
    setHash(null);
    try {
      if (needsXlm) {
        setStep("xlm");
        await stellar.friendbot(address);
        for (let i = 0; i < 20 && (await stellar.xlm(address)) === null; i++) await new Promise((r) => setTimeout(r, 1000));
        setXlm(await stellar.xlm(address));
      }
      setStep("quote");
      const q = await stellar.quoteUsdc(amount * USDC);
      setQuote(q);
      setStep("sign");
      setHash(await stellar.buyUsdc(address, q, needsTrustline));
      setStep("done");
      onFunded(amount * USDC);
    } catch (e) {
      setStep("idle");
      setErr(explain(e));
    }
  };

  const state = (s: "xlm" | "swap" | "deposit") => {
    if (s === "xlm") return !needsXlm ? "done" : step === "xlm" ? "now" : "";
    if (s === "swap") return step === "done" ? "done" : step === "quote" || step === "sign" ? "now" : "";
    return step === "done" ? "now" : "";
  };

  return (
    <div className="fund">
      <div className="field">
        <span className="flabel">
          <span>Amount</span>
          <span className="faint num">{quote ? `≈ ${Number(quote.xlm).toLocaleString("en-US", { maximumFractionDigits: 1 })} XLM on the testnet DEX` : "Quoting…"}</span>
        </span>
        <div className="seg" role="group" aria-label="USDC to get">
          {AMOUNTS.map((a) => (
            <button key={a.toString()} type="button" aria-pressed={amount === a} disabled={busy} onClick={() => setAmount(a)}>
              {a.toString()} USDC
            </button>
          ))}
        </div>
      </div>

      <ol className="steps">
        <li className={`step ${state("xlm")}`}>
          <div>
            <b>Testnet XLM</b>
            <span>{needsXlm ? "Your account is new on testnet: friendbot sends it 10,000 XLM." : xlm != null ? `${(Number(xlm) / 1e7).toLocaleString("en-US", { maximumFractionDigits: 0 })} XLM in your wallet.` : "Checking your account…"}</span>
          </div>
        </li>
        <li className={`step ${state("swap")}`}>
          <div>
            <b>{needsTrustline ? "Add USDC and buy it" : "Buy USDC"}</b>
            <span>One signature: {needsTrustline ? "the USDC trustline, then " : ""}XLM to USDC on Stellar's testnet DEX, at most 2% above the quote.</span>
          </div>
        </li>
        <li className={`step ${state("deposit")}`}>
          <div>
            <b>Deposit into the lane</b>
            <span>The deposit form opens with what arrived.</span>
          </div>
        </li>
      </ol>

      {short_ && <p className="msg warn">Your wallet has less XLM than this costs. Pick a smaller amount.</p>}
      <button className="btn stellar lg block" disabled={busy || !quote || short_} onClick={() => void run()}>
        {step === "xlm" ? "Asking friendbot…" : step === "quote" ? "Quoting…" : step === "sign" ? "Sign in your wallet…" : `Get ${amount.toString()} test USDC`}
      </button>
      {hash && <p className="msg ok">Bought {amount.toString()} USDC on Stellar ({short(hash, 6)}). Deposit it below.</p>}
      {err && (
        <p className="msg err">
          {err}{" "}
          <a href={config.faucetUrl} target="_blank" rel="noreferrer">
            Circle faucet
          </a>
        </p>
      )}
      <p className="hint">
        Testnet assets have no value. The USDC is Circle's testnet USDC (<span className="mono">{short(config.usdcAsset.split(":")[1] ?? "", 4)}</span>), the token this lane settles in.
      </p>
    </div>
  );
}
