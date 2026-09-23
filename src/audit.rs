use std::collections::{BTreeMap, HashSet};

use crate::types::*;

#[derive(Clone, Copy, Debug)]
struct Live {
    account: AccountId,
    side: Side,
    price: Price,
    remaining: u32,
    filled: u32,
    seq: u64,
}

pub struct Audit {
    cfg: Config,
    live: BTreeMap<OrderId, Live>,
    ever: HashSet<OrderId>,
    next_seq: u64,
}

macro_rules! ensure {
    ($cond:expr, $($msg:tt)*) => {
        if !$cond {
            return Err(format!($($msg)*));
        }
    };
}

type Check = Result<(), String>;

impl Audit {
    pub fn new(cfg: Config) -> Self {
        Audit {
            cfg,
            live: BTreeMap::new(),
            ever: HashSet::new(),
            next_seq: 0,
        }
    }

    pub fn live_orders(&self) -> usize {
        self.live.len()
    }

    pub fn check(&mut self, env: &Envelope, events: &[Event]) -> Check {
        self.check_command(env, events)
            .and_then(|_| self.check_not_crossed())
            .map_err(|e| format!("{e}\n  command: {env:?}\n  events:  {events:?}"))
    }

    fn best(&self, maker_side: Side) -> Option<(OrderId, Live)> {
        self.live
            .iter()
            .filter(|(_, o)| o.side == maker_side)
            .min_by_key(|(_, o)| {
                let price_rank = match maker_side {
                    Side::Buy => u16::MAX - o.price.0,
                    Side::Sell => o.price.0,
                };
                (price_rank, o.seq)
            })
            .map(|(id, o)| (*id, *o))
    }

    fn crossing(&self, side: Side, limit: Price) -> Option<(OrderId, Live)> {
        self.best(side.opposite())
            .filter(|(_, o)| side.crosses(limit, o.price))
    }

    fn owned(&self, id: OrderId, account: AccountId) -> Option<Live> {
        self.live.get(&id).filter(|o| o.account == account).copied()
    }

    fn check_not_crossed(&self) -> Check {
        if let (Some((_, b)), Some((_, a))) = (self.best(Side::Buy), self.best(Side::Sell)) {
            ensure!(
                b.price < a.price,
                "L6: book crossed: bid {:?} >= ask {:?}",
                b.price,
                a.price
            );
        }
        Ok(())
    }

    fn check_command(&mut self, env: &Envelope, events: &[Event]) -> Check {
        for e in events {
            let req = match *e {
                Event::Rejected { req, .. }
                | Event::Rested { req, .. }
                | Event::Filled { req }
                | Event::Cancelled { req, .. }
                | Event::Reduced { req, .. } => req,
                Event::Trade { taker_req, .. } => taker_req,
                Event::MakerFilled { .. } => env.req,
            };
            ensure!(
                req == env.req,
                "event carries req {req}, command req is {}",
                env.req
            );
        }

        match env.cmd {
            Command::New { side, qty, kind } => {
                if let [Event::Rejected { reason, .. }] = events {
                    return self.check_new_rejection(side, qty, kind, *reason);
                }
                ensure!(
                    self.expected_static_reject(qty, kind).is_none(),
                    "invalid order was not rejected"
                );
                self.check_flow(env, side, qty.0, kind, events)
            }
            Command::Cancel { id } => match *events {
                [
                    Event::Rejected {
                        reason: RejectReason::UnknownOrStaleId,
                        ..
                    },
                ] => {
                    ensure!(
                        self.owned(id, env.account).is_none(),
                        "cancel of a live owned order was rejected"
                    );
                    Ok(())
                }
                [
                    Event::Cancelled {
                        id: Some(got),
                        qty,
                        reason: CancelReason::User,
                        ..
                    },
                ] => {
                    let o = self
                        .owned(id, env.account)
                        .ok_or("O1: cancelled an order not owned by the sender")?;
                    ensure!(
                        got == id && qty.0 == o.remaining,
                        "cancel reported wrong id or qty"
                    );
                    self.live.remove(&id);
                    Ok(())
                }
                _ => Err("cancel produced an invalid event sequence".into()),
            },
            Command::Reduce { id, total_qty } => self.check_reduce(env, id, total_qty.0, events),
            Command::Replace {
                id,
                price,
                total_qty,
                post_only,
            } => self.check_replace(env, id, price, total_qty, post_only, events),
        }
    }

    fn expected_static_reject(&self, qty: Qty, kind: OrderKind) -> Option<RejectReason> {
        let price = match kind {
            OrderKind::Limit { price, .. } | OrderKind::PostOnly { price } => Some(price),
            OrderKind::Market { protection } => protection,
        };
        if price.is_some_and(|p| !self.cfg.price_ok(p)) {
            Some(RejectReason::InvalidPrice)
        } else if !self.cfg.qty_ok(qty) {
            Some(RejectReason::InvalidQty)
        } else {
            None
        }
    }

    fn check_new_rejection(
        &self,
        side: Side,
        qty: Qty,
        kind: OrderKind,
        reason: RejectReason,
    ) -> Check {
        if let Some(expected) = self.expected_static_reject(qty, kind) {
            ensure!(reason == expected, "expected {expected:?}, got {reason:?}");
            return Ok(());
        }
        match kind {
            OrderKind::PostOnly { price } => {
                let expected = if self.crossing(side, price).is_some() {
                    RejectReason::PostOnlyWouldCross
                } else {
                    RejectReason::BookFull
                };
                ensure!(
                    reason == expected,
                    "post-only: expected {expected:?}, got {reason:?}"
                );
            }
            OrderKind::Limit {
                price,
                tif: Tif::Gtc,
            } => {
                ensure!(
                    reason == RejectReason::BookFull,
                    "GTC rejected with {reason:?}"
                );
                ensure!(
                    self.crossing(side, price).is_none(),
                    "marketable GTC rejected as BookFull (011)"
                );
            }
            _ => {
                return Err(format!(
                    "valid IOC/market order rejected with {reason:?} (004)"
                ));
            }
        }
        Ok(())
    }

    fn check_flow(
        &mut self,
        env: &Envelope,
        side: Side,
        qty: u32,
        kind: OrderKind,
        events: &[Event],
    ) -> Check {
        let limit = self.cfg.limit_of(side, kind);
        let mut remaining = qty;
        let mut i = 0;

        while let Some(&Event::Trade {
            taker_side,
            maker_id,
            price,
            qty: q,
            maker_remaining,
            ts,
            ..
        }) = events.get(i)
        {
            ensure!(
                !matches!(kind, OrderKind::PostOnly { .. }),
                "M4: post-only traded on arrival"
            );
            ensure!(
                taker_side == side && ts == env.ts,
                "trade has wrong taker side or ts"
            );
            ensure!(remaining > 0, "trade after the taker was filled");
            let (best_id, maker) = self
                .best(side.opposite())
                .ok_or("trade against an empty side")?;
            ensure!(
                maker_id == best_id,
                "M3: traded with {maker_id:?}, best/oldest was {best_id:?}"
            );
            ensure!(
                side.crosses(limit, maker.price),
                "M2: trade beyond the taker's limit"
            );
            ensure!(
                price == maker.price,
                "M1: trade price is not the maker's price"
            );
            ensure!(maker.account != env.account, "M6: self-trade");
            let fill = remaining.min(maker.remaining);
            ensure!(q.0 == fill, "fill qty {} != min(taker, maker) {fill}", q.0);
            ensure!(
                maker_remaining.0 == maker.remaining - fill,
                "wrong maker_remaining"
            );
            remaining -= fill;
            i += 1;
            if maker_remaining.0 == 0 {
                ensure!(
                    events.get(i) == Some(&Event::MakerFilled { id: maker_id }),
                    "E3: MakerFilled must immediately follow the maker's last trade"
                );
                self.live.remove(&maker_id);
                i += 1;
            } else {
                let m = self.live.get_mut(&maker_id).expect("maker exists");
                m.remaining -= fill;
                m.filled += fill;
            }
        }

        ensure!(
            i + 1 == events.len(),
            "E2: expected exactly one terminal event after the trades"
        );
        let next = self.crossing(side, limit);
        match events[i] {
            Event::Filled { .. } => {
                ensure!(remaining == 0, "E1: Filled with {remaining} remaining");
            }
            Event::Rested { id, qty: q, .. } => {
                ensure!(rests(kind), "M5: IOC/market order rested");
                ensure!(
                    remaining > 0 && q.0 == remaining,
                    "E1: rested qty {} != remaining {remaining}",
                    q.0
                );
                ensure!(next.is_none(), "rested while crossing liquidity remained");
                ensure!(id.generation() % 2 == 1, "S2: live ID with even generation");
                ensure!(self.ever.insert(id), "S4: ID {id:?} was issued twice");
                let seq = self.next_seq;
                self.next_seq += 1;
                self.live.insert(
                    id,
                    Live {
                        account: env.account,
                        side,
                        price: limit,
                        remaining,
                        filled: 0,
                        seq,
                    },
                );
            }
            Event::Cancelled {
                id: None,
                qty: q,
                reason,
                ..
            } => {
                ensure!(
                    remaining > 0 && q.0 == remaining,
                    "E1: cancelled qty {} != remaining {remaining}",
                    q.0
                );
                match reason {
                    CancelReason::Unfilled => {
                        ensure!(!rests(kind), "GTC/post-only cancelled as Unfilled");
                        ensure!(
                            next.is_none(),
                            "IOC stopped while crossing liquidity remained"
                        );
                    }
                    CancelReason::SelfTrade => {
                        ensure!(
                            next.is_some_and(|(_, o)| o.account == env.account),
                            "SelfTrade cancel but the next maker is not the sender's"
                        );
                    }
                    CancelReason::CapacityExhausted => {
                        ensure!(
                            matches!(kind, OrderKind::Limit { tif: Tif::Gtc, .. }),
                            "C2: CapacityExhausted on a non-GTC order"
                        );
                        ensure!(next.is_none(), "CapacityExhausted before matching finished");
                    }
                    other => return Err(format!("invalid taker cancel reason {other:?}")),
                }
            }
            other => return Err(format!("invalid terminal event {other:?}")),
        }
        Ok(())
    }

    fn check_reduce(&mut self, env: &Envelope, id: OrderId, total: u32, events: &[Event]) -> Check {
        let owned = self.owned(id, env.account);
        match *events {
            [
                Event::Rejected {
                    reason: RejectReason::UnknownOrStaleId,
                    ..
                },
            ] => {
                ensure!(
                    owned.is_none(),
                    "reduce of a live owned order rejected as unknown"
                );
            }
            [
                Event::Rejected {
                    reason: RejectReason::InvalidChange,
                    ..
                },
            ] => {
                let o = owned.ok_or("InvalidChange for an unknown order")?;
                ensure!(
                    total as u64 > o.filled as u64 + o.remaining as u64,
                    "valid reduce rejected"
                );
            }
            [
                Event::Cancelled {
                    id: Some(got),
                    qty,
                    reason: CancelReason::ReduceToZero,
                    ..
                },
            ] => {
                let o = owned.ok_or("O1: reduced an order not owned by the sender")?;
                ensure!(
                    got == id && total <= o.filled && qty.0 == o.remaining,
                    "bad ReduceToZero"
                );
                self.live.remove(&id);
            }
            [
                Event::Reduced {
                    id: got,
                    filled,
                    remaining,
                    ..
                },
            ] => {
                let o = owned.ok_or("O1: reduced an order not owned by the sender")?;
                ensure!(
                    got == id && filled.0 == o.filled,
                    "Reduced reports wrong id or filled"
                );
                ensure!(
                    total > o.filled && total <= o.filled + o.remaining,
                    "O2: reduce outside the allowed range"
                );
                ensure!(
                    remaining.0 == total - o.filled,
                    "Reduced remaining != total - filled"
                );
                self.live.get_mut(&id).expect("owned").remaining = remaining.0;
            }
            _ => return Err("reduce produced an invalid event sequence".into()),
        }
        Ok(())
    }

    fn check_replace(
        &mut self,
        env: &Envelope,
        id: OrderId,
        price: Price,
        total_qty: Qty,
        post_only: bool,
        events: &[Event],
    ) -> Check {
        let static_reject = if !self.cfg.price_ok(price) {
            Some(RejectReason::InvalidPrice)
        } else if total_qty > self.cfg.max_order_qty {
            Some(RejectReason::InvalidQty)
        } else {
            None
        };
        let owned = self.owned(id, env.account);

        if let [Event::Rejected { reason, .. }] = *events {
            if let Some(expected) = static_reject {
                ensure!(
                    reason == expected,
                    "replace: expected {expected:?}, got {reason:?}"
                );
                return Ok(());
            }
            let Some(o) = owned else {
                ensure!(
                    reason == RejectReason::UnknownOrStaleId,
                    "replace: expected UnknownOrStaleId"
                );
                return Ok(());
            };
            ensure!(
                total_qty.0 > o.filled,
                "replace to total <= filled must cancel, not reject"
            );
            let expected = if post_only && self.crossing(o.side, price).is_some() {
                RejectReason::PostOnlyWouldCross
            } else {
                RejectReason::BookFull
            };
            ensure!(
                reason == expected,
                "replace: expected {expected:?}, got {reason:?}"
            );
            return Ok(());
        }

        ensure!(static_reject.is_none(), "invalid replace was not rejected");
        let o = owned.ok_or("O1: replaced an order not owned by the sender")?;
        ensure!(
            events.first()
                == Some(&Event::Cancelled {
                    req: env.req,
                    id: Some(id),
                    qty: Qty(o.remaining),
                    reason: CancelReason::Replaced,
                }),
            "replace must start with Cancelled{{Replaced}} for the old order"
        );
        self.live.remove(&id);
        let new_remaining = total_qty.0.saturating_sub(o.filled);
        if new_remaining == 0 {
            ensure!(
                events.len() == 1,
                "replace to total <= filled must only cancel"
            );
            return Ok(());
        }
        let kind = if post_only {
            OrderKind::PostOnly { price }
        } else {
            OrderKind::Limit {
                price,
                tif: Tif::Gtc,
            }
        };
        self.check_flow(env, o.side, new_remaining, kind, &events[1..])
    }
}
