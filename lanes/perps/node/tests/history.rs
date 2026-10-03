//! Candles and recent fills rebuilt from stored blocks and the history file
//! beside the store (H-15, DEC-103): a restart loses nothing.

mod common;

use common::harness::*;

use caravel_perps_node::history::History;
use caravel_testkit::lane::{BTC, BTC_PRICE};

#[test]
fn history_is_rebuilt_from_the_store_and_the_file() {
    let mut t = T::native();
    busy_lane(&mut t, 12);
    let head = t.core.height();
    let state = t.core.state();
    let dir = tempfile::tempdir().unwrap();
    let path = History::path_for(&dir.path().join("seq.sqlite"));

    // First start: nothing in the file, every stored block is replayed.
    let mut h = History::open(&path).unwrap();
    assert_eq!(h.catch_up(t.core.store(), &state, head).unwrap(), head);
    let candles = h.candles(BTC, 1, 100);
    assert!(!candles.is_empty());
    assert_eq!(candles.last().unwrap().close, BTC_PRICE.to_string());
    let fills = h.fills(BTC, 100);
    assert!(!fills.is_empty(), "the cross in busy_lane filled");
    drop(h);

    // A restart: the file holds it all, and replays only what is new.
    let mut again = History::open(&path).unwrap();
    assert_eq!(again.height(), head);
    assert_eq!(again.candles(BTC, 1, 100), candles);
    assert_eq!(again.fills(BTC, 100).len(), fills.len());
    t.prices();
    t.block();
    let head2 = t.core.height();
    assert_eq!(
        again
            .catch_up(t.core.store(), &t.core.state(), head2)
            .unwrap(),
        head2 - head
    );
    assert_eq!(
        again.fills(BTC, 100).len(),
        fills.len(),
        "no fill counted twice"
    );
}
