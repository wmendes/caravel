import { Fragment } from "react";

import { Chip } from "../components/ui";
import { config } from "../config";
import { useApp } from "../state";

/** Honest claims (spec §2.1) and what is not claimed (§2.2). */
export function About() {
  const { status } = useApp();
  return (
    <div className="page">
    <div className="prose">
      <h1>About Caravel Perps</h1>
      <p>
        A perpetual futures exchange on a Caravel lane, settled on Stellar in Circle's testnet USDC. Every number in the app says where it stands: <Chip kind="soft">soft</Chip> on the lane, or <Chip kind="settled">settled</Chip> once a checkpoint is accepted on Stellar.
      </p>
      <h3>What this testnet demo is</h3>
      <p>
        Caravel Perps runs as a Soroban lane. Every lane block is executed with the exact perps-engine Wasm through <span className="mono">soroban-env-host</span>, by the sequencer and independently by three validators. Every checkpoint, including the full block data, is posted to Stellar and accepted only with a 2-of-3 validator signature. Anyone can rebuild the lane from Stellar data alone and check every state hash. USDC stays in a Stellar contract; withdrawals need a Merkle proof against an accepted checkpoint. If the lane stops checkpointing or ignores deposits and forced-withdrawal requests sent through Stellar, anyone can freeze the contract, and users reclaim their last checkpointed equity on Stellar. This is a testnet build: the contract admin can still upgrade it and rotate validators.
      </p>
      <h3>Who runs it</h3>
      <p>
        The Caravel team runs the sequencer and all three validators, on one machine. Prices are signed by the Caravel team's oracle key. They come from Coinbase's public market data (its live trade stream, with its spot price and Reflector's testnet feed as fallbacks), at most once per block.
      </p>
      <h3>What it does not claim</h3>
      <ul>
        <li>That Stellar validators execute lane blocks. They verify signatures and store data; they do not re-run trades.</li>
        <li>That the lane is trustless. If 2 of 3 validators collude with the sequencer, they can sign a wrong state. Replay detects this but does not prevent it.</li> {/* claims-ok: states the claim to deny it (spec §2.2) */}
        <li>That lane blocks are Stellar transactions. The block time is the lane's own setting{status ? ` (${status.block_time_ms} ms here)` : ""}; Stellar ledgers stay about 5 s.</li>
        <li>Full censorship resistance for traders: a sequencer can process forced withdrawals, which only release free collateral, while ignoring a trader's closing orders.</li>
        <li>That anything here is audited, production-ready, on mainnet, or a first.</li> {/* claims-ok: lists the terms to deny them (spec §2.2) */}
      </ul>
      <h3>Contracts and nodes</h3>
      <dl className="kv wide mono">
        <dt>settlement</dt>
        <dd>
          <a href={`${config.explorerUrl}/contract/${config.settlementContract}`} target="_blank" rel="noreferrer">
            {config.settlementContract}
          </a>
        </dd>
        <dt>USDC</dt>
        <dd>{config.usdcContract}</dd>
        <dt>lane</dt>
        <dd>{status ? `${status.lane_name} (${status.lane_id})` : "…"}</dd>
        {config.validatorUrls.map((u, i) => (
          <FragmentRow key={u} k={`validator ${i + 1}`} v={`${u}/v1/status`} />
        ))}
      </dl>
      <p className="hint">
        Verify it yourself: <span className="mono">caravel-perps-node replay --rpc {config.rpcUrl} --settlement {config.settlementContract} …</span> rebuilds every checkpoint from Stellar data.
      </p>
    </div>
    </div>
  );
}

function FragmentRow({ k, v }: { k: string; v: string }) {
  return (
    <Fragment>
      <dt>{k}</dt>
      <dd>
        <a href={v} target="_blank" rel="noreferrer">
          {v}
        </a>
      </dd>
    </Fragment>
  );
}
