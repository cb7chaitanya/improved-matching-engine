use omnibook::audit::Audit;
use omnibook::reference::Reference;
use omnibook::types::*;

use CancelReason as C;
use Event::*;
use RejectReason as R;
use Side::{Buy, Sell};

struct Book {
    engine: Reference,
    audit: Audit,
    req: u64,
}

impl Book {
    fn new(cfg: Config) -> Self {
        Book {
            engine: Reference::new(cfg),
            audit: Audit::new(cfg),
            req: 0,
        }
    }

    fn small() -> Self {
        Self::new(Config {
            max_order_qty: Qty(100),
            ..Config::binary_cents(16)
        })
    }

    fn send(&mut self, account: AccountId, cmd: Command) -> Vec<Event> {
        self.req += 1;
        let env = Envelope {
            req: self.req,
            ts: 1000 + self.req,
            account,
            cmd,
        };
        let before = self.engine.clone();
        let mut out = Vec::new();
        self.engine.process(&env, &mut out);
        self.audit.check(&env, &out).unwrap();
        self.engine.check_state().unwrap();
        if let [Rejected { .. }] = out.as_slice() {
            assert_eq!(before, self.engine, "E5: rejection changed state");
        }
        out
    }

    fn rest(&mut self, account: AccountId, side: Side, price: u16, qty: u32) -> OrderId {
        match self.send(account, gtc(side, price, qty)).as_slice() {
            [Rested { id, .. }] => *id,
            other => panic!("expected to rest, got {other:?}"),
        }
    }
}

fn gtc(side: Side, price: u16, qty: u32) -> Command {
    Command::New {
        side,
        qty: Qty(qty),
        kind: OrderKind::Limit {
            price: Price(price),
            tif: Tif::Gtc,
        },
    }
}
fn ioc(side: Side, price: u16, qty: u32) -> Command {
    Command::New {
        side,
        qty: Qty(qty),
        kind: OrderKind::Limit {
            price: Price(price),
            tif: Tif::Ioc,
        },
    }
}
fn market(side: Side, qty: u32, protection: Option<u16>) -> Command {
    Command::New {
        side,
        qty: Qty(qty),
        kind: OrderKind::Market {
            protection: protection.map(Price),
        },
    }
}
fn post_only(side: Side, price: u16, qty: u32) -> Command {
    Command::New {
        side,
        qty: Qty(qty),
        kind: OrderKind::PostOnly {
            price: Price(price),
        },
    }
}
fn trade_prices(events: &[Event]) -> Vec<(OrderId, u16, u32)> {
    events
        .iter()
        .filter_map(|e| match e {
            Trade {
                maker_id,
                price,
                qty,
                ..
            } => Some((*maker_id, price.0, qty.0)),
            _ => None,
        })
        .collect()
}

#[test]
fn invalid_price_and_qty_are_rejected_price_first() {
    let mut b = Book::small();
    assert_eq!(
        b.send(1, gtc(Buy, 0, 5)),
        [Rejected {
            req: 1,
            reason: R::InvalidPrice
        }]
    );
    assert_eq!(
        b.send(1, gtc(Buy, 100, 5)),
        [Rejected {
            req: 2,
            reason: R::InvalidPrice
        }]
    );
    assert_eq!(
        b.send(1, gtc(Buy, 50, 0)),
        [Rejected {
            req: 3,
            reason: R::InvalidQty
        }]
    );
    assert_eq!(
        b.send(1, gtc(Buy, 50, 101)),
        [Rejected {
            req: 4,
            reason: R::InvalidQty
        }]
    );
    assert_eq!(
        b.send(1, gtc(Buy, 0, 0)),
        [Rejected {
            req: 5,
            reason: R::InvalidPrice
        }]
    );
}

#[test]
fn trade_happens_at_maker_price() {
    let mut b = Book::small();
    let ask = b.rest(1, Sell, 50, 5);
    let ev = b.send(2, gtc(Buy, 55, 5));
    assert_eq!(
        trade_prices(&ev),
        [(ask, 50, 5)],
        "taker gets price improvement"
    );
    assert_eq!(ev[1], MakerFilled { id: ask });
    assert_eq!(ev[2], Filled { req: 2 });
}

#[test]
fn price_then_time_priority() {
    let mut b = Book::small();
    let a50 = b.rest(1, Sell, 50, 3);
    let b50 = b.rest(2, Sell, 50, 3);
    let c49 = b.rest(3, Sell, 49, 3);
    let ev = b.send(4, gtc(Buy, 55, 7));
    assert_eq!(
        trade_prices(&ev),
        [(c49, 49, 3), (a50, 50, 3), (b50, 50, 1)]
    );
}

#[test]
fn stale_id_cannot_touch_the_order_now_in_its_slot() {
    let mut b = Book::small();
    let old = b.rest(1, Sell, 50, 1);
    b.send(2, gtc(Buy, 50, 1));
    let new = b.rest(1, Sell, 60, 1);
    assert_eq!(old.slot(), new.slot());
    assert_ne!(old.generation(), new.generation());
    assert_eq!(
        b.send(1, Command::Cancel { id: old }),
        [Rejected {
            req: 4,
            reason: R::UnknownOrStaleId
        }]
    );
    assert_eq!(b.engine.live_orders(), 1, "the new order survived");
}

#[test]
fn out_of_range_slot_is_rejected_not_a_panic() {
    let mut b = Book::small();
    let id = OrderId::new(u32::MAX - 1, 1);
    assert_eq!(
        b.send(1, Command::Cancel { id }),
        [Rejected {
            req: 1,
            reason: R::UnknownOrStaleId
        }]
    );
}

#[test]
fn post_only_at_equal_price_crosses_and_is_rejected() {
    let mut b = Book::small();
    b.rest(1, Sell, 50, 5);
    assert_eq!(
        b.send(2, post_only(Buy, 50, 5)),
        [Rejected {
            req: 2,
            reason: R::PostOnlyWouldCross
        }]
    );
    assert!(matches!(
        b.send(2, post_only(Buy, 49, 5)).as_slice(),
        [Rested { .. }]
    ));
}

#[test]
fn post_only_rests_when_opposite_side_is_empty() {
    let mut b = Book::small();
    assert!(matches!(
        b.send(1, post_only(Buy, 99, 5)).as_slice(),
        [Rested { .. }]
    ));
}

#[test]
fn market_order_into_empty_book_is_cancelled_not_rejected() {
    let mut b = Book::small();
    assert_eq!(
        b.send(1, market(Buy, 5, None)),
        [Cancelled {
            req: 1,
            id: None,
            qty: Qty(5),
            reason: C::Unfilled
        }]
    );
}

#[test]
fn market_protection_is_an_absolute_ceiling() {
    let mut b = Book::small();
    let a = b.rest(1, Sell, 50, 2);
    b.rest(1, Sell, 70, 2);
    let ev = b.send(2, market(Buy, 4, Some(60)));
    assert_eq!(trade_prices(&ev), [(a, 50, 2)]);
    assert_eq!(
        *ev.last().unwrap(),
        Cancelled {
            req: 3,
            id: None,
            qty: Qty(2),
            reason: C::Unfilled
        }
    );
}

#[test]
fn ioc_takes_no_slot_so_it_works_on_a_full_book() {
    let mut b = Book::new(Config {
        capacity: 1,
        ..Config::binary_cents(1)
    });
    b.rest(1, Sell, 50, 5);
    let ev = b.send(2, ioc(Buy, 50, 2));
    assert_eq!(ev.last(), Some(&Filled { req: 2 }));
}

#[test]
fn self_trade_cancels_newest_and_stops_matching() {
    let mut b = Book::small();
    let other = b.rest(2, Sell, 49, 1);
    let own = b.rest(1, Sell, 50, 1);
    let behind = b.rest(2, Sell, 50, 1);
    let ev = b.send(1, gtc(Buy, 55, 5));
    assert_eq!(
        trade_prices(&ev),
        [(other, 49, 1)],
        "fills before the self-match stand"
    );
    assert_eq!(
        *ev.last().unwrap(),
        Cancelled {
            req: 4,
            id: None,
            qty: Qty(4),
            reason: C::SelfTrade
        }
    );
    assert!(matches!(
        b.send(1, Command::Cancel { id: own }).as_slice(),
        [Cancelled { .. }]
    ));
    assert!(matches!(
        b.send(2, Command::Cancel { id: behind }).as_slice(),
        [Cancelled { .. }]
    ));
}

#[test]
fn wrong_owner_looks_exactly_like_unknown() {
    let mut b = Book::small();
    let id = b.rest(1, Buy, 40, 5);
    assert_eq!(
        b.send(2, Command::Cancel { id }),
        [Rejected {
            req: 2,
            reason: R::UnknownOrStaleId
        }]
    );
}

#[test]
fn reduce_uses_total_size_so_a_stale_view_cannot_over_expose() {
    let mut b = Book::small();
    let id = b.rest(1, Sell, 50, 10);
    b.send(2, gtc(Buy, 50, 3));
    assert_eq!(
        b.send(
            1,
            Command::Reduce {
                id,
                total_qty: Qty(5)
            }
        ),
        [Reduced {
            req: 3,
            id,
            filled: Qty(3),
            remaining: Qty(2)
        }]
    );
    assert_eq!(
        b.send(
            1,
            Command::Reduce {
                id,
                total_qty: Qty(5)
            }
        ),
        [Reduced {
            req: 4,
            id,
            filled: Qty(3),
            remaining: Qty(2)
        }]
    );
    assert_eq!(
        b.send(
            1,
            Command::Reduce {
                id,
                total_qty: Qty(6)
            }
        ),
        [Rejected {
            req: 5,
            reason: R::InvalidChange
        }]
    );
    assert_eq!(
        b.send(
            1,
            Command::Reduce {
                id,
                total_qty: Qty(3)
            }
        ),
        [Cancelled {
            req: 6,
            id: Some(id),
            qty: Qty(2),
            reason: C::ReduceToZero
        }]
    );
}

#[test]
fn reduce_keeps_queue_priority() {
    let mut b = Book::small();
    let first = b.rest(1, Sell, 50, 5);
    let second = b.rest(2, Sell, 50, 5);
    b.send(
        1,
        Command::Reduce {
            id: first,
            total_qty: Qty(2),
        },
    );
    let ev = b.send(3, gtc(Buy, 50, 3));
    assert_eq!(trade_prices(&ev), [(first, 50, 2), (second, 50, 1)]);
}

#[test]
fn rejected_post_only_replace_leaves_old_order_untouched() {
    let mut b = Book::small();
    b.rest(2, Sell, 50, 5);
    let id = b.rest(1, Buy, 45, 5);
    assert_eq!(
        b.send(
            1,
            Command::Replace {
                id,
                price: Price(50),
                total_qty: Qty(5),
                post_only: true
            }
        ),
        [Rejected {
            req: 3,
            reason: R::PostOnlyWouldCross
        }]
    );
    assert!(matches!(
        b.send(1, Command::Cancel { id }).as_slice(),
        [Cancelled {
            reason: C::User,
            ..
        }]
    ));
}

#[test]
fn replace_gets_a_new_id_and_loses_priority() {
    let mut b = Book::small();
    let first = b.rest(1, Sell, 50, 5);
    let second = b.rest(2, Sell, 50, 5);
    let ev = b.send(
        1,
        Command::Replace {
            id: first,
            price: Price(50),
            total_qty: Qty(5),
            post_only: false,
        },
    );
    let [
        Cancelled {
            reason: C::Replaced,
            ..
        },
        Rested { id: replaced, .. },
    ] = ev.as_slice()
    else {
        panic!("{ev:?}")
    };
    assert_ne!(*replaced, first);
    let ev = b.send(3, gtc(Buy, 50, 6));
    assert_eq!(trade_prices(&ev), [(second, 50, 5), (*replaced, 50, 1)]);
}

#[test]
fn replace_to_total_at_or_below_filled_only_cancels() {
    let mut b = Book::small();
    let id = b.rest(1, Sell, 50, 10);
    b.send(2, gtc(Buy, 50, 4));
    assert_eq!(
        b.send(
            1,
            Command::Replace {
                id,
                price: Price(60),
                total_qty: Qty(4),
                post_only: false
            }
        ),
        [Cancelled {
            req: 3,
            id: Some(id),
            qty: Qty(6),
            reason: C::Replaced
        }]
    );
}

#[test]
fn full_book_rejects_non_marketable_gtc_but_marketable_gtc_rests_in_freed_slot() {
    let mut b = Book::new(Config {
        capacity: 1,
        ..Config::binary_cents(1)
    });
    b.rest(1, Sell, 50, 2);
    assert_eq!(
        b.send(2, gtc(Buy, 40, 1)),
        [Rejected {
            req: 2,
            reason: R::BookFull
        }]
    );
    assert_eq!(
        b.send(2, post_only(Buy, 40, 1)),
        [Rejected {
            req: 3,
            reason: R::BookFull
        }]
    );
    let ev = b.send(2, gtc(Buy, 50, 5));
    assert!(
        matches!(ev.last(), Some(Rested { qty: Qty(3), .. })),
        "{ev:?}"
    );
}

#[test]
fn capacity_exhausted_when_the_freed_maker_slot_retires() {
    let mut b = Book::new(Config {
        capacity: 1,
        generation_limit: 1,
        ..Config::binary_cents(1)
    });
    b.rest(1, Sell, 50, 2);
    let ev = b.send(2, gtc(Buy, 50, 5));
    assert_eq!(
        *ev.last().unwrap(),
        Cancelled {
            req: 2,
            id: None,
            qty: Qty(3),
            reason: C::CapacityExhausted
        }
    );
    assert_eq!(b.engine.retired_slots(), 1);
}

#[test]
fn replace_is_rejected_when_the_old_slot_would_retire_on_a_full_book() {
    let mut b = Book::new(Config {
        capacity: 1,
        generation_limit: 1,
        ..Config::binary_cents(1)
    });
    let id = b.rest(1, Sell, 50, 2);
    assert_eq!(
        b.send(
            1,
            Command::Replace {
                id,
                price: Price(55),
                total_qty: Qty(2),
                post_only: false
            }
        ),
        [Rejected {
            req: 2,
            reason: R::BookFull
        }]
    );
    assert_eq!(b.engine.live_orders(), 1, "old order untouched");
}
