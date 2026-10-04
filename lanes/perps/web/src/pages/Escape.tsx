/**
 * Exit (spec §13.6, §18.2, DEC-106): how a user leaves the lane, whatever the
 * lane does. While it runs, the page shows the two timers that let anyone
 * freeze the settlement contract and the user's escape proof, ready to
 * download. Once frozen, each account claims its share of the last
 * checkpointed equity on Stellar, with a proof from the sequencer, any
 * validator, or a file from `caravel-perps-node replay --prove-escape`.
 */
import { useState } from "react";

import { lane, type Proof } from "../api/lane";
import { explain, stellar, type ExitParams } from "../api/stellar";
import { Chip, Empty, Skel, useNow } from "../components/ui";
import { config } from "../config";
import { short, usdc } from "../format";
import { Link } from "../router";
import { useApp, usePoll } from "../state";

type SourcedProof = Proof & { from: string };

export function Escape() {
  const { frozen, address, connect, onChain } = useApp();
  const now = useNow(1000);
  const [params, setParams] = useState<ExitParams | null>(null);
  const [oldest, setOldest] = useState<bigint | null | undefined>(undefined);
  const [info, setInfo] = useState<{ payout_num: bigint; payout_den: bigint } | null>(null);
  const [proof, setProof] = useState<SourcedProof | null>(null);
  const [claimed, setClaimed] = useState(false);
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<{ ok: boolean; text: string } | null>(null);
  const [refunds, setRefunds] = useState<{ index: bigint; amount: bigint }[]>([]);

  usePoll(async () => {
    setParams(await stellar.params());
    if (onChain) setOldest(await stellar.oldestUnprocessed(onChain.inbox_through));
  }, 30000, [onChain?.seq]);

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
        setProof({ ...(await lane.escapeProof(address, base)), from: name });
        return;
      } catch {
        /* try the next source */
      }
    }
    setMsg({ ok: false, text: "No node served a proof for this account. If it has never deposited, it has nothing to escape with. Otherwise, build one with caravel-perps-node replay --prove-escape." });
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

  const download = () => {
    if (!proof) return;
    const blob = new Blob([JSON.stringify(proof, null, 2) + "\n"], { type: "application/json" });
    const a = document.createElement("a");
    a.href = URL.createObjectURL(blob);
    a.download = `escape-proof-${proof.account ?? "account"}-cp${proof.seq}.json`;
    a.click();
    URL.revokeObjectURL(a.href);
  };

  // The two freeze conditions, in seconds (the contract's own arithmetic).
  const sinceAccepted = onChain ? Math.max(0, Math.floor(now / 1000) - Number(onChain.accepted_at)) : null;
  const sinceQueued = oldest ? Math.max(0, Math.floor(now / 1000) - Number(oldest)) : oldest === null ? null : undefined;
  const escapeLimit = params ? Number(params.escape_timeout_secs) : null;
  const inclusionLimit = params ? Number(params.force_inclusion_window_secs) : null;
  const canFreeze = !frozen && ((sinceAccepted !== null && escapeLimit !== null && sinceAccepted > escapeLimit) || (typeof sinceQueued === "number" && inclusionLimit !== null && sinceQueued > inclusionLimit));
  const share = proof && info && info.payout_den > 0n ? (BigInt(proof.equity ?? "0") * info.payout_num) / info.payout_den : null;

  return (
    <div className="page">
      <header className="page-head">
        <div>
          <h1>Exit</h1>
          <p>Your USDC is in a contract on Stellar, not with the lane. These are the ways out, and the timers that keep the lane honest.</p>
        </div>
        {frozen ? <Chip kind="danger">contract frozen</Chip> : <Chip kind="settled">contract live</Chip>}
      </header>

      {frozen && (
        <section className="section danger-zone" aria-label="The contract is frozen">
          <div className="section-body">
            <h2>The settlement contract is frozen</h2>
            <p className="dim" style={{ maxWidth: "72ch" }}>
              The lane stopped checkpointing or ignored a request sent through Stellar, so nothing more is accepted from it. Each account claims its equity from the last accepted checkpoint{onChain ? ` (#${onChain.seq})` : ""}, pro rata to what the vault holds.
              {info && ` The vault covers ${info.payout_den > 0n ? `${Number((info.payout_num * 10_000n) / info.payout_den) / 100}%` : "none"} of checkpointed equity.`}
            </p>
          </div>
        </section>
      )}

      <section className="board" aria-label="Freeze timers">
        <div className="timers">
          <Timer
            k="Since the last accepted checkpoint"
            value={sinceAccepted}
            limit={escapeLimit}
            ok="Checkpoints are landing on Stellar."
            over="Past the escape timeout: anyone can freeze the contract."
            rule="If no checkpoint is accepted for this long, anyone can freeze the contract."
          />
          <Timer
            k="Oldest request the lane hasn't processed"
            value={sinceQueued === undefined ? undefined : sinceQueued}
            limit={inclusionLimit}
            ok="The lane is processing deposits and forced withdrawals."
            none="No deposit or forced withdrawal is waiting."
            over="Past the force-inclusion window: anyone can freeze the contract."
            rule="A deposit or forced withdrawal sent through Stellar must be processed within this window."
          />
        </div>
        {canFreeze && address && (
          <div className="section-body">
            <p className="msg warn">A freeze condition holds. Freezing stops the lane for everyone and opens escape claims.</p>
            <div>
              <button
                className="btn danger-ghost"
                disabled={busy}
                onClick={async () => {
                  setBusy(true);
                  setMsg(null);
                  try {
                    await stellar.freeze(address);
                    setMsg({ ok: true, text: "Frozen on Stellar. Claim your equity below." });
                  } catch (e) {
                    setMsg({ ok: false, text: explain(e) });
                  } finally {
                    setBusy(false);
                  }
                }}
              >
                Freeze the settlement contract
              </button>
            </div>
          </div>
        )}
      </section>

      <div className="cols wide">
        <section className="section" aria-label="Your escape proof">
          <div className="section-head">
            <h3>Your escape proof</h3>
            {proof && <Chip kind="settled">checkpoint #{proof.seq}</Chip>}
          </div>
          {!address ? (
            <Empty
              title="Connect the wallet that owns the lane account"
              action={
                <button className="btn stellar" onClick={() => void connect()}>
                  Connect wallet
                </button>
              }
            />
          ) : (
            <div className="section-body">
              <p className="hint">
                A Merkle proof of your equity in the last checkpoint accepted on Stellar. {frozen ? "Claim with it below." : "You can't claim with it while the contract is live, but you can keep a copy: if every node went away, it is all you need after a freeze."}
              </p>
              <div className="row-btns">
                <button className="btn" onClick={() => void fetchProof()}>
                  {proof ? "Refresh my proof" : "Get my proof"}
                </button>
                <label className="btn">
                  Load a proof file
                  <input type="file" accept="application/json" hidden onChange={(e) => e.target.files?.[0] && void upload(e.target.files[0])} />
                </label>
                {proof && (
                  <button className="btn" onClick={download}>
                    Download (.json)
                  </button>
                )}
              </div>
              {proof && (
                <dl className="kv">
                  <dt>From</dt>
                  <dd>{proof.from}</dd>
                  <dt>Checkpoint</dt>
                  <dd>#{proof.seq}</dd>
                  <dt>Equity in it</dt>
                  <dd className="ink">{usdc(proof.equity ?? "0")} USDC</dd>
                  {frozen && (
                    <>
                      <dt>You receive</dt>
                      <dd className="ink">{share !== null ? `${usdc(share)} USDC` : "–"}</dd>
                    </>
                  )}
                </dl>
              )}
              {frozen &&
                (claimed ? (
                  <p className="msg ok">You have already claimed.</p>
                ) : (
                  <button
                    className="btn stellar lg block"
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
                ))}
              {msg && <p className={`msg ${msg.ok ? "ok" : "err"}`}>{msg.text}</p>}
            </div>
          )}
        </section>

        <section className="section" aria-label="Ways out">
          <div className="section-head">
            <h3>Three ways out</h3>
          </div>
          <ol className="ways">
            <li>
              <b>Withdraw</b>
              <span>The normal way. The lane releases free collateral; you claim it on Stellar after the next checkpoint, about a minute.</span>
              <Link to="/portfolio">Portfolio</Link>
            </li>
            <li>
              <b>Forced withdrawal through Stellar</b>
              <span>If the lane ignores your withdrawals. You ask the contract directly; the lane must process it within the force-inclusion window or anyone can freeze it.</span>
              <Link to="/portfolio">Portfolio, at the bottom</Link>
            </li>
            <li>
              <b>Escape after a freeze</b>
              <span>If the lane stops. You claim your equity from the last accepted checkpoint with your escape proof, pro rata to the vault.</span>
              <span className="faint">This page</span>
            </li>
          </ol>
          {frozen && (
            <div className="section-body">
              <span className="label">Deposits the lane never processed</span>
              {!address ? (
                <p className="hint">Connect to see yours.</p>
              ) : refunds.length === 0 ? (
                <p className="hint">None for your account.</p>
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
            </div>
          )}
        </section>
      </div>
    </div>
  );
}

function dur(s: number): string {
  if (s < 90) return `${s} s`;
  if (s < 5400) return `${Math.round(s / 60)} min`;
  return `${(s / 3600).toFixed(1)} h`;
}

function Timer({ k, value, limit, ok, over, none, rule }: { k: string; value: number | null | undefined; limit: number | null; ok: string; over: string; none?: string; rule: string }) {
  const pct = value != null && limit ? Math.min(100, (value / limit) * 100) : 0;
  const state = value == null ? "idle" : limit && value > limit ? "crit" : pct > 50 ? "hot" : "";
  return (
    <div className="timer">
      <span className="k">{k}</span>
      <span className="v num">
        {value === undefined || limit === null ? <Skel w={90} h={20} /> : value === null ? "none" : dur(value)}
        {limit !== null && <span className="faint"> / {dur(limit)}</span>}
      </span>
      <div className={`meter ${state}`} role="meter" aria-label={k} aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(pct)}>
        <i style={{ width: `${pct}%` }} />
      </div>
      <span className="sub">{value === null ? (none ?? ok) : state === "crit" ? over : ok}</span>
      <span className="faint rule">{rule}</span>
    </div>
  );
}
