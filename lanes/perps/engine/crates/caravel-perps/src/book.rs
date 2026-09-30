//! Order book helpers. Bids are best first by price descending, asks by price
//! ascending; ties keep order_id ascending (INV-P3). A new order always has the
//! highest order_id, so it goes after every resting order at its price.

use alloc::vec::Vec;

use caravel_types::state::OrderV1;
use caravel_types::tx::Side;

/// Inserts a resting order on `side`, keeping price-time priority.
pub(crate) fn insert(book: &mut Vec<OrderV1>, side: Side, order: OrderV1) {
    let at = match side {
        Side::Buy => book.iter().position(|o| o.price < order.price),
        Side::Sell => book.iter().position(|o| o.price > order.price),
    }
    .unwrap_or(book.len());
    book.insert(at, order);
}

/// Whether a taker at `price` on `side` crosses the best opposite order.
pub(crate) fn crosses(opposite: &[OrderV1], side: Side, price: i64) -> bool {
    opposite.first().is_some_and(|best| match side {
        Side::Buy => best.price <= price,
        Side::Sell => best.price >= price,
    })
}
