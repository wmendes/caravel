/**
 * How it works (spec §2.1, §2.2, DEC-106): who does what, who you trust for
 * what and how to check it, what is not claimed, and the contracts and nodes.
 */
import { useState } from "react";

import { Chip } from "../components/ui";
import { config } from "../config";
import { ago, short } from "../format";
import { useApp } from "../state";

export function About() {
  const { status, onChain } = useApp();
  const ms = status?.block_time_ms;
  const blockTime = ms ? (ms >= 1000 ? `${ms / 1000} s` : `${ms} ms`) : "…";
  const every = status?.checkpoint_every_blocks;
  const cadence = ms && every ? Math.round((ms * every) / 1000) : null;
  const threshold = status?.signers?.threshold ?? 2;
  const n = status?.signers?.validators.length ?? 3;

  return (
    <div className="page">
      <header className="page-head">
        <div>
          <h1>How it works</h1>
          <p>
            Caravel Perps is a perpetual futures exchange on a Caravel lane: its own chain for trading, settled on Stellar in Circle's testnet USDC. Every number in the app says where it stands, <Chip kind="soft">soft</Chip> on the lane or <Chip kind="settled">settled</Chip> on Stellar.
          </p>
        </div>
      </header>

      <section className="section" aria-label="How a trade reaches Stellar">
        <ol className="diagram">
          <li className="node you">
            <span className="label">You</span>
            <b>Stellar wallet</b>
            <span>Deposits and claims on Stellar; signs orders, or lets a trading key on this device sign them.</span>
          </li>
          <li className="edge">
            <span>orders</span>
          </li>
          <li className="node soft">
            <span className="label">Lane · soft</span>
            <b>Sequencer</b>
            <span>Orders them and runs the perps engine (Wasm) on every block, every {blockTime}. Fills are final on the lane at once.</span>
          </li>
          <li className="edge">
            <span>blocks</span>
          </li>
          <li className="node soft">
            <span className="label">Lane · soft</span>
            <b>{n} validators</b>
            <span>Each re-runs every block through the same engine Wasm and signs a checkpoint header if every state hash matches.</span>
          </li>
          <li className="edge">
            <span>{threshold} of {n} signatures</span>
          </li>
          <li className="node settled">
            <span className="label">Stellar · settled</span>
            <b>Settlement contract</b>
            <span>Holds the USDC. Accepts a checkpoint, with its full block data, only with enough validator signatures. Pays withdrawals against a Merkle proof.</span>
          </li>
        </ol>
      </section>

      <section className="board" aria-label="This lane now">
        <div className="statrow">
          <div className="stat">
            <span className="k">Block time</span>
            <span className="v num">{blockTime}</span>
            <span className="sub">a setting of this lane, not of Stellar</span>
          </div>
          <div className="stat">
            <span className="k">Checkpoints</span>
            <span className="v num">{every ? `every ${every} blocks` : "…"}</span>
            <span className="sub">{cadence ? `about every ${cadence >= 60 ? `${Math.round(cadence / 60)} min` : `${cadence} s`}` : ""}</span>
          </div>
          <div className="stat">
            <span className="k">Signers</span>
            <span className="v num">
              {threshold} of {n}
            </span>
            <span className="sub">validators per checkpoint</span>
          </div>
          <div className="stat">
            <span className="k">
              Last accepted <Chip kind="settled">settled</Chip>
            </span>
            <span className="v num">{onChain ? `#${onChain.seq}` : "…"}</span>
            <span className="sub">{onChain ? `on Stellar ${ago(Number(onChain.accepted_at) * 1000)}` : ""}</span>
          </div>
        </div>
      </section>

      <section className="section" aria-label="What you trust">
        <div className="section-head">
          <h3>What you trust, and how to check it</h3>
        </div>
        <div className="table-wrap">
          <table className="table trust">
            <thead>
              <tr>
                <th>For</th>
                <th>You trust</th>
                <th>You can check</th>
              </tr>
            </thead>
            <tbody>
              <tr>
                <td>Your USDC</td>
                <td>The settlement contract on Stellar</td>
                <td>It is a public contract; balances, checkpoints and claims are on Stellar.</td>
              </tr>
              <tr>
                <td>Correct execution</td>
                <td>
                  {threshold} of {n} validators not colluding with the sequencer
                </td>
                <td>Anyone can replay every block from Stellar data alone and compare each state hash.</td>
              </tr>
              <tr>
                <td>Order and inclusion</td>
                <td>The sequencer, for your orders</td>
                <td>Deposits and forced withdrawals sent through Stellar must be processed in time, or anyone can freeze the contract.</td>
              </tr>
              <tr>
                <td>Prices</td>
                <td>The Caravel team's oracle key</td>
                <td>Every price is signed and recorded in a block; the source is Coinbase market data, with Reflector as a fallback.</td>
              </tr>
              <tr>
                <td>Upgrades</td>
                <td>The contract admin (testnet)</td>
                <td>The admin can still upgrade the contract and rotate validators on this testnet build.</td>
              </tr>
            </tbody>
          </table>
        </div>
      </section>

      <div className="cols">
        <section className="section" aria-label="Not claimed">
          <div className="section-head">
            <h3>What this does not claim</h3>
          </div>
          <ul className="claims">
            <li>That Stellar validators execute lane blocks. They verify signatures and store data; they do not re-run trades.</li>
            <li>That the lane is trustless. If {threshold} of {n} validators collude with the sequencer, they can sign a wrong state. Replay detects this but does not prevent it.</li> {/* claims-ok: states the claim to deny it (spec §2.2) */}
            <li>That lane blocks are Stellar transactions. Stellar ledgers stay about 5 s.</li>
            <li>Full censorship resistance for traders: a sequencer can process forced withdrawals, which only release free collateral, while ignoring a trader's closing orders.</li>
            <li>That anything here is audited, production-ready, on mainnet, or a first.</li> {/* claims-ok: lists the terms to deny them (spec §2.2) */}
            <li>That one machine is decentralized: the Caravel team runs the sequencer and all three validators, on one VM.</li>
          </ul>
        </section>

        <section className="section" aria-label="Contracts and nodes">
          <div className="section-head">
            <h3>Contracts and nodes</h3>
          </div>
          <dl className="kv wide refs">
            <Ref k="Settlement" v={config.settlementContract} href={`${config.explorerUrl}/contract/${config.settlementContract}`} />
            <Ref k="USDC" v={config.usdcContract} href={`${config.explorerUrl}/contract/${config.usdcContract}`} />
            <Ref k="Lane id" v={status?.lane_id ?? "…"} label={status?.lane_name} />
            {config.validatorUrls.map((u, i) => (
              <Ref key={u} k={`Validator ${i + 1}`} v={`${u}/v1/status`} href={`${u}/v1/status`} />
            ))}
          </dl>
          <div className="section-body">
            <p className="hint">
              Verify it yourself: <span className="mono">caravel-perps-node replay --rpc {config.rpcUrl} --settlement {short(config.settlementContract, 6)} …</span> rebuilds every checkpoint from Stellar data.
            </p>
          </div>
        </section>
      </div>
    </div>
  );
}

function Ref({ k, v, href, label }: { k: string; v: string; href?: string; label?: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <>
      <dt>{k}</dt>
      <dd>
        <span className="ref">
          {href ? (
            <a className="mono" href={href} target="_blank" rel="noreferrer">
              {v.startsWith("http") ? v.replace(/^https?:\/\//, "") : short(v, 10)}
            </a>
          ) : (
            <span className="mono">{short(v, 10)}</span>
          )}
          {label && <span className="faint">{label}</span>}
          <button
            type="button"
            className="btn ghost sm"
            onClick={() =>
              void navigator.clipboard?.writeText(v).then(() => {
                setCopied(true);
                setTimeout(() => setCopied(false), 1200);
              })
            }
          >
            {copied ? "Copied" : "Copy"}
          </button>
        </span>
      </dd>
    </>
  );
}
