import { useState } from "react";

import { lane } from "../api/lane";
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
  const { status } = useApp();
  const [blocks, setBlocks] = useState<BlockView[]>([]);
  const [checkpoints, setCheckpoints] = useState<CheckpointView[]>([]);
  const [open, setOpen] = useState<string | null>(null);

  usePoll(async () => {
    const s = status ?? (await lane.status());
    const h = Number(s.height);
    const bs = await Promise.all(Array.from({ length: Math.min(10, h) }, (_, i) => lane.block(h - i).catch(() => null)));
    setBlocks(bs.filter(Boolean) as unknown as BlockView[]);
    const last = Number(s.checkpoints.sequenced ?? 0);
    const cs = await Promise.all(Array.from({ length: Math.min(8, last) }, (_, i) => lane.checkpoint(last - i).catch(() => null)));
    setCheckpoints(cs.filter(Boolean) as unknown as CheckpointView[]);
  }, 4000, [status?.height === undefined]);

  const signerKey = (i: number) => status?.signers?.validators.find((v) => v.index === i)?.key;

  return (
    <>
      <div className="page-head">
        <div>
          <h1>Explorer</h1>
          <p>Lane blocks come from the sequencer every second. Checkpoints carry the blocks to Stellar, signed by at least {status?.signers?.threshold ?? 2} of {status?.signers?.validators.length ?? 3} validators.</p>
        </div>
      </div>
      <div className="two">
        <section className="panel">
          <h3>Latest lane blocks</h3>
          <table>
            <thead>
              <tr>
                <th>Block</th>
                <th>Entries</th>
                <th>State hash</th>
                <th className="r">Time</th>
              </tr>
            </thead>
            <tbody>
              {blocks.map((b) => (
                <tr key={b.height}>
                  <td>
                    <span className="badge lane">#{b.height}</span> {b.checkpoint_end && <span className="note">ends checkpoint</span>}
                  </td>
                  <td className="num">{b.entries.length}</td>
                  <td className="mono">{short(b.state_hash_after, 6)}</td>
                  <td className="r muted small">{new Date(Number(b.timestamp_ms)).toLocaleTimeString()}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </section>
        <section className="panel">
          <h3>Checkpoints</h3>
          {checkpoints.length === 0 ? (
            <p className="empty">No checkpoint yet</p>
          ) : (
            <table>
              <thead>
                <tr>
                  <th>Seq</th>
                  <th>Blocks</th>
                  <th>Status</th>
                  <th className="r">Stellar</th>
                </tr>
              </thead>
              <tbody>
                {checkpoints.map((c) => (
                  <tr key={c.seq} onClick={() => setOpen(open === c.seq ? null : c.seq)} style={{ cursor: "pointer" }}>
                    <td>
                      <span className="badge harbor">#{c.seq}</span>
                    </td>
                    <td className="num">
                      {c.first_block_height}–{c.last_block_height}
                    </td>
                    <td>{c.status === "accepted" ? "accepted on Stellar" : c.status}</td>
                    <td className="r">
                      {c.stellar_tx_hash ? (
                        <a href={`${config.explorerUrl}/tx/${c.stellar_tx_hash}`} target="_blank" rel="noreferrer" onClick={(e) => e.stopPropagation()}>
                          {short(c.stellar_tx_hash, 5)}
                        </a>
                      ) : (
                        "–"
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </section>
      </div>
      {open &&
        (() => {
          const c = checkpoints.find((x) => x.seq === open);
          if (!c) return null;
          return (
            <section className="panel" style={{ marginTop: 16 }}>
              <div className="spread">
                <h2>Checkpoint #{c.seq}</h2>
                <button className="btn small" onClick={() => setOpen(null)}>
                  Close
                </button>
              </div>
              <div className="two" style={{ marginTop: 12 }}>
                <dl className="kv mono">
                  <dt>header hash</dt>
                  <dd>{c.header_hash}</dd>
                  {Object.entries(c.header).map(([k, v]) => (
                    <FragmentKV key={k} k={k} v={String(v)} />
                  ))}
                  <dt>batch</dt>
                  <dd>{c.batch_bytes.toLocaleString()} bytes</dd>
                </dl>
                <div>
                  <h3>Validator signatures</h3>
                  <table>
                    <tbody>
                      {c.signatures.map((s) => (
                        <tr key={s.signer_index}>
                          <td>#{s.signer_index}</td>
                          <td className="mono">{short(signerKey(s.signer_index) ?? "?", 6)}</td>
                          <td className="mono muted">{short(s.signature, 8)}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                  <p className="note" style={{ marginTop: 12 }}>
                    Anyone can rebuild this checkpoint from Stellar alone with <span className="mono">caravel-perps-node replay</span>.
                  </p>
                </div>
              </div>
            </section>
          );
        })()}
    </>
  );
}

function FragmentKV({ k, v }: { k: string; v: string }) {
  return (
    <>
      <dt>{k.replace(/_/g, " ")}</dt>
      <dd>{v}</dd>
    </>
  );
}
