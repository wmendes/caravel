//! `caravel-node`: the sequencer, validator and replay binary (spec §14–§16).

fn main() {
    eprintln!(
        "caravel-node {}: no subcommands yet (genesis lands in T-003, sequencer in T-007)",
        env!("CARGO_PKG_VERSION")
    );
    std::process::exit(2);
}
