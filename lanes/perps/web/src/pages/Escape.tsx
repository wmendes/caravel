import { useState } from "react";

import { lane, type Proof } from "../api/lane";
import { explain, stellar } from "../api/stellar";
import { config } from "../config";
import { Chip, Empty } from "../components/ui";
import { short, usdc } from "../format";
import { useApp, usePoll } from "../state";

/**
 * The escape hatch (spec §13.6, §18.2): when the settlement contract is
 * frozen, each account claims its share of the last checkpointed equity on
 * Stellar, with a proof from the sequencer, any validator, or a JSON file from
 * `caravel-perps-node replay --prove-escape`.
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
    setMsg({ ok: false, text: "No source served a proof. Upload one from caravel-perps-node replay --prove-escape." });
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
      <div className="page">
        <div className="page-head">
          <div>
            <h1>Escape</h1>
            <p>
              The settlement contract is not frozen <Chip kind="settled">normal</Chip>. Withdraw from the Portfolio page as usual.
            </p>
          </div>
        </div>
        <section className="section">
          <div className="section-head">
            <h3>When this page matters</h3>
          </div>
          <div className="section-body">
            <p className="dim" style={{ maxWidth: "72ch" }}>
              If the lane stops checkpointing for the escape timeout, or leaves a deposit or forced withdrawal sent through Stellar unprocessed past the force-inclusion window, anyone can freeze the contract. From then on, each account claims its equity from the last accepted checkpoint here, with a proof from the sequencer, any validator, or a file from <span className="mono">caravel-perps-node replay --prove-escape</span>.
            </p>
          </div>
        </section>
      </div>
    );
  }

  const share = proof && info && info.payout_den > 0n ? (BigInt(proof.equity ?? "0") * info.payout_num) / info.payout_den : null;
  return (
    <div className="page">
      <div className="page-head">
        <div>
          <h1>The settlement contract is frozen</h1>
          <p>
            The lane stopped checkpointing or ignored a request sent through Stellar, so nothing more is accepted from it. Each account can claim its equity from the last accepted checkpoint{onChain ? ` (#${onChain.seq})` : ""}, pro rata to what the vault holds.
          </p>
        </div>
        <Chip kind="danger">frozen</Chip>
      </div>
      {!address ? (
        <section className="section">
          <Empty
            title="Connect the wallet that owns the lane account"
            action={
              <button className="btn stellar" onClick={() => void connect()}>
                Connect wallet
              </button>
            }
          />
        </section>
      ) : (
        <div className="cols">
          <section className="section">
            <div className="section-head">
              <h3>Your escape claim</h3>
              {info && <span className="hint num">Payout {info.payout_den > 0n ? `${Number((info.payout_num * 10_000n) / info.payout_den) / 100}%` : "none"}</span>}
            </div>
            <div className="section-body">
              {info && (
                <p className="hint">
                  The vault holds {usdc(info.payout_num)} USDC against {usdc(info.payout_den)} USDC of checkpointed equity.
                </p>
              )}
              {claimed ? (
                <p className="msg ok">You have already claimed.</p>
              ) : (
                <>
                  <div className="row" style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
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
                      <dd className="ink">{share !== null ? `${usdc(share)} USDC` : "–"}</dd>
                    </dl>
                  )}
                  <button
                    className="btn stellar block"
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
                    {busy ? "Sign in your wallet…" : "Claim on Stellar"}
                  </button>
                </>
              )}
              {msg && <p className={`msg ${msg.ok ? "ok" : "err"}`}>{msg.text}</p>}
            </div>
          </section>
          <section className="section">
            <div className="section-head">
              <h3>Deposits the lane never processed</h3>
            </div>
            {refunds.length === 0 ? (
              <Empty title="None for your account">A deposit the lane never processed would show here, refundable 1:1.</Empty>
            ) : (
              <table className="table">
                <tbody>
                  {refunds.map((r) => (
                    <tr key={r.index.toString()}>
                      <td className="num">Inbox #{r.index.toString()}</td>
                      <td className="r num">{usdc(r.amount)} USDC</td>
                      <td className="r">
                        <button
                          className="btn sm stellar"
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
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
          </section>
        </div>
      )}
    </div>
  );
}
