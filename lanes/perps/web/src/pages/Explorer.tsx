/**
 * The lane, live: blocks as the sequencer makes them (soft), and the
 * checkpoints that carry them to Stellar through sealed, signed and accepted
 * (settled). Search finds a block, a checkpoint or an account (DEC-106).
 */
import { Fragment, useEffect, useState, type FormEvent } from "react";

import { lane, stream, type Account, type Status } from "../api/lane";
import { Chip, Empty, Skel, useNow } from "../components/ui";
import { config } from "../config";
import { ago, short, usdc } from "../format";
import { useApp } from "../state";

interface BlockRow {
  height: string;
  timestamp_ms: string;
  entries: number;
  checkpoint_end: boolean;
  state_hash: string;
}

interface BlockView {
  height: string;
  timestamp_ms: string;
  checkpoint_end: boolean;
  block_hash: string;
  state_hash_after: string;
  entries: { index: number; type: string; code: number | null }[];
}

interface CheckpointView {
  seq: string;
  status: string;
  header_hash: string;
  header: Record<string, string | number>;
  batch_bytes: number;
  first_block_height: string;
  last_block_height: string;
  signatures: { signer_index: number; signature: string }[];
  stellar_tx_hash: string | null;
  stellar_ledger: number | null;
}

type Found = { kind: "block"; v: BlockView } | { kind: "checkpoint"; v: CheckpointView } | { kind: "account"; g: string; v: Account | null };

const KEEP = 40;

const toRow = (b: BlockView): BlockRow => ({ height: b.height, timestamp_ms: b.timestamp_ms, entries: b.entries.length, checkpoint_end: b.checkpoint_end, state_hash: b.state_hash_after });

/** The union of two block lists, newest first, the latest KEEP. */
function merge(a: BlockRow[] | null, b: BlockRow[]): BlockRow[] {
  const by = new Map<string, BlockRow>();
  for (const r of [...(a ?? []), ...b]) by.set(r.height, r);
  return [...by.values()].sort((x, y) => Number(y.height) - Number(x.height)).slice(0, KEEP);
}

export function Explorer() {
  const { status, onChain } = useApp();
  const now = useNow(500);
  const [blocks, setBlocks] = useState<BlockRow[] | null>(null);
  const [checkpoints, setCheckpoints] = useState<CheckpointView[] | null>(null);
  const [fresh, setFresh] = useState<string | null>(null);
  const [found, setFound] = useState<Found | null>(null);
  const [query, setQuery] = useState("");
  const [searchErr, setSearchErr] = useState<string | null>(null);

  // The latest blocks, then every block as it is made.
  useEffect(() => {
    let live = true;
    void lane
      .status()
      .then(async (s) => {
        const h = Number(s.height);
        const bs = await Promise.all(Array.from({ length: Math.min(20, h) }, (_, i) => lane.block(h - i).catch(() => null)));
        if (!live) return;
        setBlocks((prev) => merge(prev, (bs.filter(Boolean) as unknown as BlockView[]).map(toRow)));
      })
      .catch(() => live && setBlocks([]));
    const stop = stream({ blocks: true }, (m) => {
      if (m.type === "block") {
        const b = m as unknown as BlockRow;
        setBlocks((prev) => {
          // Blocks made between the first fetch and the stream's first message are fetched once.
          const top = prev?.[0] ? Number(prev[0].height) : null;
          if (top !== null && Number(b.height) - top > 1) {
            const missing = Array.from({ length: Math.min(KEEP, Number(b.height) - top - 1) }, (_, i) => top + 1 + i);
            void Promise.all(missing.map((h) => lane.block(h).catch(() => null))).then((got) => live && setBlocks((p) => merge(p, (got.filter(Boolean) as unknown as BlockView[]).map(toRow))));
          }
          return merge(prev, [b]);
        });
        setFresh(b.height);
      }
      if (m.type === "checkpoint") void refreshCheckpoints(setCheckpoints);
    });
    return () => {
      live = false;
      stop();
    };
  }, []);

  useEffect(() => {
    void refreshCheckpoints(setCheckpoints);
    const t = setInterval(() => void refreshCheckpoints(setCheckpoints), 10_000);
    return () => clearInterval(t);
  }, []);

  const search = async (e: FormEvent) => {
    e.preventDefault();
    const q = query.trim();
    setSearchErr(null);
    try {
      if (/^G[A-Z2-7]{55}$/.test(q)) setFound({ kind: "account", g: q, v: await lane.account(q) });
      else if (/^c\s*#?\d+$/i.test(q)) setFound({ kind: "checkpoint", v: (await lane.checkpoint(q.replace(/\D/g, ""))) as unknown as CheckpointView });
      else if (/^#?\d+$/.test(q)) setFound({ kind: "block", v: (await lane.block(q.replace(/\D/g, ""))) as unknown as BlockView });
      else setSearchErr("Search a block height (12345), a checkpoint (c42) or an account (G…).");
    } catch (err) {
      setSearchErr(err instanceof Error ? err.message : String(err));
    }
  };

  const openBlock = async (h: string) => {
    try {
      setFound({ kind: "block", v: (await lane.block(h)) as unknown as BlockView });
    } catch (err) {
      setSearchErr(err instanceof Error ? err.message : String(err));
    }
  };

  const threshold = status?.signers?.threshold ?? 2;
  const nValidators = status?.signers?.validators.length ?? 3;
  const blockTime = status ? (status.block_time_ms >= 1000 ? `${status.block_time_ms / 1000} s` : `${status.block_time_ms} ms`) : "…";
  const acceptedAt = onChain ? Number(onChain.accepted_at) * 1000 : null;

  return (
    <div className="page">
      <header className="page-head">
        <div>
          <h1>Explorer</h1>
          <p>
            A block every {blockTime} on the lane. Every {status?.checkpoint_every_blocks ?? "…"} blocks a checkpoint carries them to Stellar, once {threshold} of {nValidators} validators have re-executed and signed it.
          </p>
        </div>
        <form className="search" onSubmit={(e) => void search(e)} role="search">
          <div className="input plain">
            <input aria-label="Search the lane" placeholder="Block, c + checkpoint, or G… account" value={query} onChange={(e) => setQuery(e.target.value)} />
          </div>
          <button className="btn" type="submit">
            Search
          </button>
        </form>
      </header>
      {searchErr && <p className="msg err">{searchErr}</p>}

      <Pipeline status={status} acceptedAt={acceptedAt} now={now} threshold={threshold} nValidators={nValidators} />

      {found && <Detail found={found} onClose={() => setFound(null)} signerKey={(i) => status?.signers?.validators.find((v) => v.index === i)?.key} />}

      <div className="cols wide">
        <section className="section">
          <div className="section-head">
            <h3>Blocks</h3>
            <span className="live-dot hint">live · soft until checkpointed</span>
          </div>
          {blocks === null ? (
            <div className="section-body">
              <Skel w="100%" />
              <Skel w="90%" />
              <Skel w="95%" />
            </div>
          ) : blocks.length === 0 ? (
            <Empty title="No blocks to show">The lane API did not answer. The status bar says when it is back.</Empty>
          ) : (
            <div className="table-wrap blocks-feed">
              <table className="table">
                <thead>
                  <tr>
                    <th>Block</th>
                    <th className="r">Entries</th>
                    <th>State hash</th>
                    <th className="r">Age</th>
                  </tr>
                </thead>
                <tbody>
                  {blocks.map((b) => (
                    <tr key={b.height} className={`clickable ${b.height === fresh ? "fresh-row" : ""}`} tabIndex={0} onClick={() => void openBlock(b.height)} onKeyDown={(e) => e.key === "Enter" && void openBlock(b.height)}>
                      <td>
                        <span className="num">#{b.height}</span> {b.checkpoint_end && <Chip kind="plain">ends checkpoint</Chip>}
                      </td>
                      <td className="r num">{b.entries > 0 ? b.entries : <span className="faint">0</span>}</td>
                      <td className="mono dim">{short(b.state_hash, 8)}</td>
                      <td className="r faint num">{ago(Number(b.timestamp_ms), now)}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </section>

        <section className="section">
          <div className="section-head">
            <h3>Checkpoints</h3>
            <span className="hint">Select one for its header and signatures</span>
          </div>
          {checkpoints === null ? (
            <div className="section-body">
              <Skel w="100%" />
              <Skel w="90%" />
            </div>
          ) : checkpoints.length === 0 ? (
            <Empty title="No checkpoint yet">The first one follows after {status?.checkpoint_every_blocks ?? "a few"} blocks.</Empty>
          ) : (
            <div className="table-wrap">
              <table className="table">
                <thead>
                  <tr>
                    <th>Seq</th>
                    <th>Blocks</th>
                    <th>Where it is</th>
                    <th className="r">Stellar tx</th>
                  </tr>
                </thead>
                <tbody>
                  {checkpoints.map((c) => (
                    <tr key={c.seq} className="clickable" tabIndex={0} onClick={() => setFound({ kind: "checkpoint", v: c })} onKeyDown={(e) => e.key === "Enter" && setFound({ kind: "checkpoint", v: c })}>
                      <td className="num">#{c.seq}</td>
                      <td className="num dim">
                        {c.first_block_height}–{c.last_block_height}
                      </td>
                      <td>
                        <StatusChip status={c.status} sigs={c.signatures.length} />
                      </td>
                      <td className="r">
                        {c.stellar_tx_hash ? (
                          <a className="mono" href={`${config.explorerUrl}/tx/${c.stellar_tx_hash}`} target="_blank" rel="noreferrer" onClick={(e) => e.stopPropagation()}>
                            {short(c.stellar_tx_hash, 5)} ↗
                          </a>
                        ) : (
                          <span className="faint">–</span>
                        )}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </section>
      </div>
    </div>
  );
}

async function refreshCheckpoints(set: (v: CheckpointView[]) => void) {
  try {
    const s = await lane.status();
    const last = Number(s.checkpoints.sequenced ?? 0);
    const cs = await Promise.all(Array.from({ length: Math.min(10, last) }, (_, i) => lane.checkpoint(last - i).catch(() => null)));
    set(cs.filter(Boolean) as unknown as CheckpointView[]);
  } catch {
    /* the status bar reports an unreachable API */
  }
}

function StatusChip({ status, sigs }: { status: string; sigs: number }) {
  if (status === "accepted") return <Chip kind="settled">accepted on Stellar</Chip>;
  if (status === "signed") return <Chip kind="soft">signed by {sigs} · submitting</Chip>;
  return <Chip kind="soft">sealed · collecting signatures</Chip>;
}

/** Where the lane's latest work stands, left to right: block, sealed, signed, accepted. */
function Pipeline({ status, acceptedAt, now, threshold, nValidators }: { status: Status | null; acceptedAt: number | null; now: number; threshold: number; nValidators: number }) {
  const blockAge = status ? Math.max(0, now - Number(status.last_block_timestamp_ms)) : null;
  const stages = [
    { k: "Latest block", v: status ? `#${status.height}` : null, sub: blockAge !== null ? `${(blockAge / 1000).toFixed(1)} s ago` : "", tone: "soft", hint: "Executed by the sequencer: soft" },
    { k: "Sealed", v: status ? `#${status.checkpoints.sequenced ?? "–"}` : null, sub: `a checkpoint every ${status?.checkpoint_every_blocks ?? "…"} blocks`, tone: "soft", hint: "A batch of blocks closed into a checkpoint header" },
    { k: "Signed", v: status ? `#${status.checkpoints.signed ?? "–"}` : null, sub: `${threshold} of ${nValidators} validators re-executed it`, tone: "soft", hint: "Validators re-ran every block through the engine Wasm and signed the header" },
    { k: "Accepted on Stellar", v: status ? `#${status.checkpoints.accepted ?? "–"}` : null, sub: acceptedAt ? ago(acceptedAt, now) : "", tone: "settled", hint: "The settlement contract checked the signatures and stored the header: settled" },
  ];
  return (
    <ol className="pipeline" aria-label="How lane work reaches Stellar">
      {stages.map((s) => (
        <li key={s.k} className={`stage ${s.tone}`} title={s.hint}>
          <span className="k">{s.k}</span>
          <span className="v num">{s.v ?? <Skel w={70} h={18} />}</span>
          <span className="sub">{s.sub}</span>
        </li>
      ))}
    </ol>
  );
}

function Detail({ found, onClose, signerKey }: { found: Found; onClose: () => void; signerKey: (i: number) => string | undefined }) {
  const title = found.kind === "block" ? `Block #${found.v.height}` : found.kind === "checkpoint" ? `Checkpoint #${found.v.seq}` : `Account ${short(found.g, 6)}`;
  return (
    <section className="section detail" aria-label={title}>
      <div className="section-head">
        <h3>{title}</h3>
        <button className="btn ghost sm" onClick={onClose}>
          Close
        </button>
      </div>
      <div className="section-body">
        {found.kind === "block" && (
          <div className="cols">
            <dl className="kv wide mono">
              <dt>time</dt>
              <dd>{new Date(Number(found.v.timestamp_ms)).toISOString()}</dd>
              <dt>block hash</dt>
              <dd>{found.v.block_hash}</dd>
              <dt>state hash after</dt>
              <dd>{found.v.state_hash_after}</dd>
              <dt>ends checkpoint</dt>
              <dd>{found.v.checkpoint_end ? "yes" : "no"}</dd>
            </dl>
            {found.v.entries.length === 0 ? (
              <p className="hint">An empty block: the clock moved and nothing else. The lane makes one every block time either way.</p>
            ) : (
              <table className="table">
                <thead>
                  <tr>
                    <th>#</th>
                    <th>Entry</th>
                    <th className="r">Result</th>
                  </tr>
                </thead>
                <tbody>
                  {found.v.entries.map((e) => (
                    <tr key={e.index}>
                      <td className="num">{e.index}</td>
                      <td>{e.type}</td>
                      <td className="r">{e.code === null || e.code === 0 ? <span className="dim">ok</span> : <span className="down">rejected ({e.code})</span>}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
          </div>
        )}
        {found.kind === "checkpoint" && (
          <div className="cols">
            <dl className="kv wide mono">
              <dt>status</dt>
              <dd>{found.v.status}</dd>
              <dt>blocks</dt>
              <dd>
                {found.v.first_block_height}–{found.v.last_block_height}
              </dd>
              <dt>header hash</dt>
              <dd>{found.v.header_hash}</dd>
              {Object.entries(found.v.header).map(([k, v]) => (
                <Fragment key={k}>
                  <dt>{k.replace(/_/g, " ")}</dt>
                  <dd>{String(v)}</dd>
                </Fragment>
              ))}
              <dt>batch</dt>
              <dd>{found.v.batch_bytes.toLocaleString()} bytes</dd>
              {found.v.stellar_ledger ? (
                <>
                  <dt>Stellar ledger</dt>
                  <dd>{found.v.stellar_ledger}</dd>
                </>
              ) : null}
            </dl>
            <div style={{ display: "grid", gap: 12, alignContent: "start" }}>
              <span className="label">Validator signatures</span>
              {found.v.signatures.length === 0 ? (
                <p className="hint">None yet: the sequencer is collecting them.</p>
              ) : (
                <table className="table">
                  <tbody>
                    {found.v.signatures.map((s) => (
                      <tr key={s.signer_index}>
                        <td className="num">#{s.signer_index}</td>
                        <td className="mono">{short(signerKey(s.signer_index) ?? "?", 6)}</td>
                        <td className="mono faint">{short(s.signature, 8)}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              )}
              <p className="hint">
                Anyone can rebuild this checkpoint from Stellar alone with <span className="mono">caravel-perps-node replay</span>.
              </p>
            </div>
          </div>
        )}
        {found.kind === "account" &&
          (found.v === null ? (
            <p className="hint">No lane account yet: this address has never deposited.</p>
          ) : (
            <dl className="kv wide">
              <dt>collateral</dt>
              <dd className="num">{usdc(found.v.collateral)} USDC</dd>
              <dt>equity</dt>
              <dd className="num">{usdc(found.v.equity)} USDC</dd>
              <dt>free collateral</dt>
              <dd className="num">{usdc(found.v.free_collateral)} USDC</dd>
              <dt>positions</dt>
              <dd>{found.v.positions.length ? found.v.positions.map((p) => `${p.symbol} ${p.lots > 0 ? "long" : "short"} ${Math.abs(p.lots)} lots`).join(", ") : "none"}</dd>
              <dt>open orders</dt>
              <dd>{found.v.open_orders.length}</dd>
              <dt>trading keys</dt>
              <dd>{found.v.session_keys.length}</dd>
              <dt>next nonce</dt>
              <dd className="num">{found.v.next_nonce}</dd>
            </dl>
          ))}
      </div>
    </section>
  );
}
