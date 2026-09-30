import { useState } from "react";

import { lane, type Proof } from "../api/lane";
import { explain, stellar } from "../api/stellar";
import { config } from "../config";
import { short, usdc } from "../format";
import { useApp, usePoll } from "../state";

/**
 * The escape hatch (spec §13.6, §18.2): when the settlement contract is
 * frozen, each account claims its share of the last checkpointed equity on
 * Stellar, with a proof from the sequencer, any validator, or a JSON file from
 * `caravel-node replay --prove-escape`.
 */
export function Escape() {
  const { frozen, address, connect, onChain } = useApp();
  const [info, setInfo] = useState<{ payout_num: bigint; payout_den: bigint } | null>(null);
  const [proof, setProof] = useState<(Proof & { from: string }) | null>(null);
  const [claimed, setClaimed] = useState(false);
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<{ ok: boolean; text: string } | null>(null);
  const [refunds, setRefunds] = useState<{ index: bigint; amount: bigint }[]>([]);

  usePoll(async () => {
    if (!frozen) return;
    setInfo(await stellar.frozenInfo());
    if (!address) return;
    setClaimed(await stellar.escapeClaimed(address));
    // Deposits the lane never processed can be refunded 1:1.
    if (onChain) {
      const count = await stellar.inboxCount();
      const mine: { index: bigint; amount: bigint }[] = [];
      for (let i = onChain.inbox_through; i < count; i++) {
        const m = await stellar.inbox(i);
        if (m && m.kind === 0 && !m.refunded && m.from === address) mine.push({ index: i, amount: m.amount });
      }
      setRefunds(mine);
    }
  }, 15000, [frozen, address, onChain?.seq]);

  const fetchProof = async () => {
    if (!address) return;
    setMsg(null);
    for (const [name, base] of [["the sequencer", config.sequencerUrl], ...config.validatorUrls.map((u, i) => [`validator ${i + 1}`, u] as const)] as const) {
      try {
        const p = await lane.escapeProof(address, base);
        setProof({ ...p, from: name });
        return;
      } catch {
        /* try the next source */
      }
    }
    setMsg({ ok: false, text: "No source served a proof. Upload one from caravel-node replay --prove-escape." });
  };

  const upload = async (f: File) => {
    try {
      const p = JSON.parse(await f.text()) as Proof;
      if (!Array.isArray(p.proof) || p.equity === undefined) throw new Error("not an escape proof");
      setProof({ ...p, from: f.name });
    } catch (e) {
      setMsg({ ok: false, text: `That file is not an escape proof: ${String(e)}` });
    }
  };

  if (!frozen) {
    return (
      <div className="prose">
        <h1>Escape</h1>
        <p>The settlement contract is not frozen. Withdraw normally from the Portfolio page.</p>
        <p className="muted">
          If the lane stops checkpointing for the escape timeout, or leaves a deposit or forced withdrawal sent through Stellar unprocessed past the force-inclusion window, anyone can freeze the contract. Then each account claims its last checkpointed equity here.
        </p>
      </div>
    );
  }

  const share = proof && info && info.payout_den > 0n ? (BigInt(proof.equity ?? "0") * info.payout_num) / info.payout_den : null;
  return (
    <>
      <div className="page-head">
        <div>
          <h1>The settlement contract is frozen</h1>
          <p>
            The lane stopped checkpointing or ignored a request sent through Stellar. Nothing more is accepted from the lane. Each account can claim its equity from the last accepted checkpoint{onChain ? ` (#${onChain.seq})` : ""}, pro rata to what the vault holds.
          </p>
        </div>
      </div>
      {!address ? (
        <button className="btn harbor" onClick={() => void connect()}>
          Connect Freighter
        </button>
      ) : (
        <div className="two">
          <section className="panel stack">
            <h3>Your escape claim</h3>
            {info && (
              <p className="note">
                Payout ratio {usdc(info.payout_num)} / {usdc(info.payout_den)} USDC ({info.payout_den > 0n ? `${Number((info.payout_num * 10_000n) / info.payout_den) / 100}%` : "no equity"}).
              </p>
            )}
            {claimed ? (
              <p className="ok">You have already claimed.</p>
            ) : (
              <>
                <div className="row">
                  <button className="btn" onClick={() => void fetchProof()}>
                    Get my proof
                  </button>
                  <label className="btn">
                    Upload a proof file
                    <input type="file" accept="application/json" hidden onChange={(e) => e.target.files?.[0] && void upload(e.target.files[0])} />
                  </label>
                </div>
                {proof && (
                  <dl className="kv">
                    <dt>Source</dt>
                    <dd>{proof.from}</dd>
                    <dt>Checkpoint</dt>
                    <dd>#{proof.seq}</dd>
                    <dt>Equity</dt>
                    <dd>{usdc(proof.equity ?? "0")} USDC</dd>
                    <dt>You receive</dt>
                    <dd>{share !== null ? `${usdc(share)} USDC` : "–"}</dd>
                  </dl>
                )}
                <button
                  className="btn harbor"
                  disabled={!proof || busy}
                  onClick={async () => {
                    if (!proof) return;
                    setBusy(true);
                    setMsg(null);
                    try {
                      const hash = await stellar.escapeClaim(address, { index: proof.index, equity: proof.equity ?? "0", proof: proof.proof });
                      setMsg({ ok: true, text: `Claimed on Stellar (${short(hash, 6)}).` });
                      setClaimed(true);
                    } catch (e) {
                      setMsg({ ok: false, text: explain(e) });
                    } finally {
                      setBusy(false);
                    }
                  }}
                >
                  {busy ? "Waiting for Freighter…" : "Claim on Stellar"}
                </button>
              </>
            )}
            {msg && <p className={msg.ok ? "ok" : "error"}>{msg.text}</p>}
          </section>
          <section className="panel stack">
            <h3>Deposits the lane never processed</h3>
            {refunds.length === 0 ? (
              <p className="empty">None for your account.</p>
            ) : (
              refunds.map((r) => (
                <div className="spread" key={r.index.toString()}>
                  <span>
                    Inbox #{r.index.toString()}: {usdc(r.amount)} USDC
                  </span>
                  <button
                    className="btn small harbor"
                    onClick={async () => {
                      try {
                        await stellar.refund(address, r.index);
                        setRefunds((x) => x.filter((y) => y.index !== r.index));
                      } catch (e) {
                        setMsg({ ok: false, text: explain(e) });
                      }
                    }}
                  >
                    Refund
                  </button>
                </div>
              ))
            )}
          </section>
        </div>
      )}
    </>
  );
}
