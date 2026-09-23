//! Original engine from cb7chaitanya/matching-engine@1934d9e, benchmark baseline only.
#![allow(dead_code, clippy::let_and_return)]

pub mod models {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Side {
        Buy,
        Sell,
    }

    #[derive(Debug, Clone)]
    pub struct Order {
        pub id: u64,
        pub symbol: String,
        pub side: Side,
        pub price: u64,
        pub qty: u64,
        pub remaining_qty: u64,
        pub timestamp: u64,
    }

    #[derive(Debug, Clone)]
    pub struct Fill {
        pub maker_order_id: u64,
        pub taker_order_id: u64,
        pub symbol: String,
        pub price: u64,
        pub qty: u64,
        pub timestamp: u64,
    }
}

pub mod orderbook {
    use std::collections::BTreeMap;
    use std::collections::VecDeque;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::models::{Fill, Order, Side};

    pub struct OrderBook {
        pub symbol: String,
        pub bids: BTreeMap<u64, VecDeque<Order>>,
        pub asks: BTreeMap<u64, VecDeque<Order>>,
    }

    impl OrderBook {
        pub fn new(symbol: String) -> Self {
            Self {
                symbol,
                bids: BTreeMap::new(),
                asks: BTreeMap::new(),
            }
        }

        pub fn add_order(&mut self, order: Order) {
            let book = match order.side {
                Side::Buy => &mut self.bids,
                Side::Sell => &mut self.asks,
            };
            book.entry(order.price).or_default().push_back(order);
        }

        pub fn match_order(&mut self, order: &mut Order) -> Vec<Fill> {
            let fills = match order.side {
                Side::Buy => Self::match_buy(order, &self.symbol, &mut self.asks),
                Side::Sell => Self::match_sell(order, &self.symbol, &mut self.bids),
            };
            fills
        }

        fn match_buy(
            taker: &mut Order,
            symbol: &str,
            asks: &mut BTreeMap<u64, VecDeque<Order>>,
        ) -> Vec<Fill> {
            let mut fills = Vec::new();
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64;

            let matchable_prices: Vec<u64> = asks.range(..=taker.price).map(|(&p, _)| p).collect();

            for price in matchable_prices {
                if taker.remaining_qty == 0 {
                    break;
                }

                let queue = match asks.get_mut(&price) {
                    Some(q) => q,
                    None => continue,
                };

                while taker.remaining_qty > 0 {
                    let maker = match queue.front_mut() {
                        Some(m) => m,
                        None => break,
                    };

                    let fill_qty = taker.remaining_qty.min(maker.remaining_qty);

                    taker.remaining_qty -= fill_qty;
                    maker.remaining_qty -= fill_qty;

                    fills.push(Fill {
                        maker_order_id: maker.id,
                        taker_order_id: taker.id,
                        symbol: symbol.to_string(),
                        price: maker.price, // trade at maker's (resting) price
                        qty: fill_qty,
                        timestamp: now,
                    });

                    // Maker fully filled — remove from queue
                    if maker.remaining_qty == 0 {
                        queue.pop_front();
                    }
                }

                // Remove empty price level
                if asks.get(&price).is_some_and(|q| q.is_empty()) {
                    asks.remove(&price);
                }
            }

            fills
        }

        fn match_sell(
            taker: &mut Order,
            symbol: &str,
            bids: &mut BTreeMap<u64, VecDeque<Order>>,
        ) -> Vec<Fill> {
            let mut fills = Vec::new();
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64;

            let matchable_prices: Vec<u64> =
                bids.range(taker.price..).rev().map(|(&p, _)| p).collect();

            for price in matchable_prices {
                if taker.remaining_qty == 0 {
                    break;
                }

                let queue = match bids.get_mut(&price) {
                    Some(q) => q,
                    None => continue,
                };

                while taker.remaining_qty > 0 {
                    let maker = match queue.front_mut() {
                        Some(m) => m,
                        None => break,
                    };

                    let fill_qty = taker.remaining_qty.min(maker.remaining_qty);

                    taker.remaining_qty -= fill_qty;
                    maker.remaining_qty -= fill_qty;

                    fills.push(Fill {
                        maker_order_id: maker.id,
                        taker_order_id: taker.id,
                        symbol: symbol.to_string(),
                        price: maker.price, // trade at maker's (resting) price
                        qty: fill_qty,
                        timestamp: now,
                    });

                    if maker.remaining_qty == 0 {
                        queue.pop_front();
                    }
                }

                if bids.get(&price).is_some_and(|q| q.is_empty()) {
                    bids.remove(&price);
                }
            }

            fills
        }
    }
}

/// `EngineState::process_order` from engine.rs, verbatim logic.
pub fn process_order(
    books: &mut std::collections::HashMap<String, orderbook::OrderBook>,
    mut order: models::Order,
) -> (models::Order, Vec<models::Fill>) {
    let book = books
        .entry(order.symbol.clone())
        .or_insert_with(|| orderbook::OrderBook::new(order.symbol.clone()));

    let fills = book.match_order(&mut order);

    if order.remaining_qty > 0 {
        book.add_order(order.clone());
    }

    (order, fills)
}
