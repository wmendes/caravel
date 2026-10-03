import { Fragment, useState } from "react";

import { lane } from "../api/lane";
import { Chip, Empty, Skel } from "../components/ui";
import { config } from "../config";
import { short } from "../format";
import { useApp, usePoll } from "../state";

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

export function Explorer() {
  const { status, onChain } = useApp();
  const [blocks, setBlocks] = useState<BlockView[] | null>(null);
  const [checkpoints, setCheckpoints] = useState<CheckpointView[] | null>(null);
  const [open, setOpen] = useState<string | null>(null);

  usePoll(async () => {
    const s = status ?? (await lane.status());
    const h = Number(s.height);
    const bs = await Promise.all(Array.from({ length: Math.min(12, h) }, (_, i) => lane.block(h - i).catch(() => null)));
    setBlocks(bs.filter(Boolean) as unknown as BlockView[]);
    const last = Number(s.checkpoints.sequenced ?? 0);
    const cs = await Promise.all(Array.from({ length: Math.min(10, last) }, (_, i) => lane.checkpoint(last - i).catch(() => null)));
    setCheckpoints(cs.filter(Boolean) as unknown as CheckpointView[]);
  }, 4000, [status?.height === undefined]);

  const signerKey = (i: number) => status?.signers?.validators.find((v) => v.index === i)?.key;
  const threshold = status?.signers?.threshold;
  const nValidators = status?.signers?.validators.length;
  const blockTime = status ? (status.block_time_ms >= 1000 ? `${status.block_time_ms / 1000} s` : `${status.block_time_ms} ms`) : "…";
  const c = open ? checkpoints?.find((x) => x.seq === open) : undefined;

  return (
    <div className="page">
      <div className="page-head">
        <div>
          <h1>Explorer</h1>
          <p>
            The sequencer makes a lane block every {blockTime}. Every {status?.checkpoint_every_blocks ?? "…"} blocks, a checkpoint carries them to Stellar, signed by at least {threshold ?? "…"} of {nValidators ?? "…"} validators.
          </p>
        </div>
      </div>

      <div className="board">
        <div className="statrow">
          <div className="stat">
            <span className="k">
              Lane height <Chip kind="soft">soft</Chip>
            </span>
            <span className="v num">{status ? `#${status.height}` : <Skel w={80} h={18} />}</span>
          </div>
          <div className="stat">
            <span className="k">Checkpoints signed</span>
            <span className="v num">{status ? `#${status.checkpoints.signed ?? "–"}` : <Skel w={80} h={18} />}</span>
          </div>
          <div className="stat">
            <span className="k">
              Accepted on Stellar <Chip kind="settled">settled</Chip>
            </span>
            <span className="v num">{onChain ? `#${onChain.seq}` : <Skel w={80} h={18} />}</span>
          </div>
          <div className="stat">
            <span className="k">Validator quorum</span>
            <span className="v num">{threshold ? `${threshold} of ${nValidators}` : "–"}</span>
          </div>
        </div>
      </div>

      <div className="cols wide">
        <section className="section">
          <div className="section-head">
            <h3>Latest lane blocks</h3>
            <Chip kind="soft">soft until checkpointed</Chip>
          </div>
          {blocks === null ? (
            <div className="section-body">
              <Skel w="100%" />
              <Skel w="90%" />
              <Skel w="95%" />
            </div>
          ) : (
            <div className="table-wrap">
              <table className="table">
                <thead>
                  <tr>
                    <th>Block</th>
                    <th className="r">Txs</th>
                    <th>State hash</th>
                    <th className="r">Time</th>
                  </tr>
                </thead>
                <tbody>
                  {blocks.map((b) => (
                    <tr key={b.height}>
                      <td>
                        <span className="num">#{b.height}</span> {b.checkpoint_end && <Chip kind="plain">ends checkpoint</Chip>}
                      </td>
                      <td className="r num">{b.entries.length}</td>
                      <td className="mono dim">{short(b.state_hash_after, 8)}</td>
                      <td className="r faint">{new Date(Number(b.timestamp_ms)).toLocaleTimeString("en-GB")}</td>
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
                    <th>Status</th>
                    <th className="r">Stellar tx</th>
                  </tr>
                </thead>
                <tbody>
                  {checkpoints.map((x) => (
                    <tr
                      key={x.seq}
                      className="clickable"
                      tabIndex={0}
                      aria-expanded={open === x.seq}
                      onClick={() => setOpen(open === x.seq ? null : x.seq)}
                      onKeyDown={(e) => (e.key === "Enter" || e.key === " ") && (e.preventDefault(), setOpen(open === x.seq ? null : x.seq))}
                    >
                      <td className="num">#{x.seq}</td>
                      <td className="num dim">
                        {x.first_block_height}–{x.last_block_height}
                      </td>
                      <td>{x.status === "accepted" ? <Chip kind="settled">accepted on Stellar</Chip> : <Chip kind="soft">{x.status}</Chip>}</td>
                      <td className="r">
                        {x.stellar_tx_hash ? (
                          <a className="mono" href={`${config.explorerUrl}/tx/${x.stellar_tx_hash}`} target="_blank" rel="noreferrer" onClick={(e) => e.stopPropagation()}>
                            {short(x.stellar_tx_hash, 5)} ↗
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

      {c && (
        <section className="section" aria-label={`Checkpoint ${c.seq}`}>
          <div className="section-head">
            <h3>Checkpoint #{c.seq}</h3>
            <button className="btn ghost sm" onClick={() => setOpen(null)}>
              Close
            </button>
          </div>
          <div className="section-body">
            <div className="cols">
              <dl className="kv wide mono">
                <dt>header hash</dt>
                <dd>{c.header_hash}</dd>
                {Object.entries(c.header).map(([k, v]) => (
                  <Fragment key={k}>
                    <dt>{k.replace(/_/g, " ")}</dt>
                    <dd>{String(v)}</dd>
                  </Fragment>
                ))}
                <dt>batch</dt>
                <dd>{c.batch_bytes.toLocaleString()} bytes</dd>
              </dl>
              <div style={{ display: "grid", gap: 12, alignContent: "start" }}>
                <span className="label">Validator signatures</span>
                <table className="table">
                  <tbody>
                    {c.signatures.map((s) => (
                      <tr key={s.signer_index}>
                        <td className="num">#{s.signer_index}</td>
                        <td className="mono">{short(signerKey(s.signer_index) ?? "?", 6)}</td>
                        <td className="mono faint">{short(s.signature, 8)}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
                <p className="hint">
                  Anyone can rebuild this checkpoint from Stellar alone with <span className="mono">caravel-perps-node replay</span>.
                </p>
              </div>
            </div>
          </div>
        </section>
      )}
    </div>
  );
}
