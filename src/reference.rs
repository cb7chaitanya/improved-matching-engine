use crate::types::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Resting {
    id: OrderId,
    account: AccountId,
    side: Side,
    price: Price,
    remaining: u32,
    filled: u32,
    seq: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reference {
    cfg: Config,
    generations: Vec<u32>,
    free: Vec<u32>,
    retired: u32,
    orders: Vec<Resting>,
    next_seq: u64,
}

impl Reference {
    pub fn new(cfg: Config) -> Self {
        cfg.validate().expect("invalid config");
        Reference {
            cfg,
            generations: vec![0; cfg.capacity as usize],
            free: (0..cfg.capacity).rev().collect(),
            retired: 0,
            orders: Vec::new(),
            next_seq: 0,
        }
    }

    pub fn live_orders(&self) -> usize {
        self.orders.len()
    }
    pub fn free_slots(&self) -> usize {
        self.free.len()
    }
    pub fn retired_slots(&self) -> u32 {
        self.retired
    }

    fn alloc(&mut self) -> Option<OrderId> {
        let slot = self.free.pop()?;
        let g = &mut self.generations[slot as usize];
        *g += 1;
        Some(OrderId::new(slot, *g))
    }

    fn will_retire(&self, id: OrderId) -> bool {
        self.generations[id.slot() as usize] as u64 + 2 > self.cfg.generation_limit as u64
    }

    fn release(&mut self, id: OrderId) {
        let slot = id.slot();
        if self.will_retire(id) {
            self.generations[slot as usize] -= 1;
            self.retired += 1;
        } else {
            self.generations[slot as usize] += 1;
            self.free.push(slot);
        }
    }

    fn best(&self, maker_side: Side) -> Option<usize> {
        let mut best: Option<usize> = None;
        for (i, o) in self.orders.iter().enumerate() {
            if o.side != maker_side {
                continue;
            }
            best = match best {
                None => Some(i),
                Some(b) => {
                    let cur = &self.orders[b];
                    let better_price = match maker_side {
                        Side::Buy => o.price > cur.price,
                        Side::Sell => o.price < cur.price,
                    };
                    if better_price || (o.price == cur.price && o.seq < cur.seq) {
                        Some(i)
                    } else {
                        Some(b)
                    }
                }
            };
        }
        best
    }

    fn marketable(&self, side: Side, limit: Price) -> bool {
        self.best(side.opposite())
            .is_some_and(|i| side.crosses(limit, self.orders[i].price))
    }

    fn lookup(&self, id: OrderId, account: AccountId) -> Option<usize> {
        if id.slot() >= self.cfg.capacity {
            return None;
        }
        self.orders
            .iter()
            .position(|o| o.id == id && o.account == account)
    }

    fn new_order(
        &mut self,
        env: &Envelope,
        side: Side,
        qty: Qty,
        kind: OrderKind,
        out: &mut impl EventSink,
    ) {
        let reject = |reason| Event::Rejected {
            req: env.req,
            reason,
        };
        let price = match kind {
            OrderKind::Limit { price, .. } | OrderKind::PostOnly { price } => Some(price),
            OrderKind::Market { protection } => protection,
        };
        if price.is_some_and(|p| !self.cfg.price_ok(p)) {
            return out.emit(reject(RejectReason::InvalidPrice));
        }
        if !self.cfg.qty_ok(qty) {
            return out.emit(reject(RejectReason::InvalidQty));
        }
        self.submit(env, side, qty.0, kind, out);
    }

    fn submit(
        &mut self,
        env: &Envelope,
        side: Side,
        qty: u32,
        kind: OrderKind,
        out: &mut impl EventSink,
    ) {
        let req = env.req;
        let limit = self.cfg.limit_of(side, kind);
        let marketable = self.marketable(side, limit);

        match kind {
            OrderKind::PostOnly { .. } => {
                if marketable {
                    return out.emit(Event::Rejected {
                        req,
                        reason: RejectReason::PostOnlyWouldCross,
                    });
                }
                if self.free.is_empty() {
                    return out.emit(Event::Rejected {
                        req,
                        reason: RejectReason::BookFull,
                    });
                }
            }
            OrderKind::Limit { tif: Tif::Gtc, .. } => {
                if self.free.is_empty() && !marketable {
                    return out.emit(Event::Rejected {
                        req,
                        reason: RejectReason::BookFull,
                    });
                }
            }
            _ => {}
        }

        let mut remaining = qty;
        while remaining > 0 {
            let Some(i) = self.best(side.opposite()) else {
                break;
            };
            if !side.crosses(limit, self.orders[i].price) {
                break;
            }
            if self.orders[i].account == env.account {
                return out.emit(Event::Cancelled {
                    req,
                    id: None,
                    qty: Qty(remaining),
                    reason: CancelReason::SelfTrade,
                });
            }
            let maker = &mut self.orders[i];
            let fill = remaining.min(maker.remaining);
            remaining -= fill;
            maker.remaining -= fill;
            maker.filled += fill;
            out.emit(Event::Trade {
                taker_req: req,
                taker_side: side,
                maker_id: maker.id,
                price: maker.price,
                qty: Qty(fill),
                maker_remaining: Qty(maker.remaining),
                ts: env.ts,
            });
            if maker.remaining == 0 {
                let maker = self.orders.remove(i);
                self.release(maker.id);
                out.emit(Event::MakerFilled { id: maker.id });
            }
        }

        if remaining == 0 {
            out.emit(Event::Filled { req });
        } else if !rests(kind) {
            out.emit(Event::Cancelled {
                req,
                id: None,
                qty: Qty(remaining),
                reason: CancelReason::Unfilled,
            });
        } else if let Some(id) = self.alloc() {
            self.orders.push(Resting {
                id,
                account: env.account,
                side,
                price: limit,
                remaining,
                filled: 0,
                seq: self.next_seq,
            });
            self.next_seq += 1;
            out.emit(Event::Rested {
                req,
                id,
                qty: Qty(remaining),
            });
        } else {
            out.emit(Event::Cancelled {
                req,
                id: None,
                qty: Qty(remaining),
                reason: CancelReason::CapacityExhausted,
            });
        }
    }

    fn cancel(&mut self, env: &Envelope, id: OrderId, out: &mut impl EventSink) {
        let Some(i) = self.lookup(id, env.account) else {
            return out.emit(Event::Rejected {
                req: env.req,
                reason: RejectReason::UnknownOrStaleId,
            });
        };
        let o = self.orders.remove(i);
        self.release(o.id);
        out.emit(Event::Cancelled {
            req: env.req,
            id: Some(id),
            qty: Qty(o.remaining),
            reason: CancelReason::User,
        });
    }

    fn reduce(&mut self, env: &Envelope, id: OrderId, total_qty: Qty, out: &mut impl EventSink) {
        let req = env.req;
        let Some(i) = self.lookup(id, env.account) else {
            return out.emit(Event::Rejected {
                req,
                reason: RejectReason::UnknownOrStaleId,
            });
        };
        let o = self.orders[i];
        let current_total = o.filled as u64 + o.remaining as u64;
        let total = total_qty.0 as u64;
        if total <= o.filled as u64 {
            self.orders.remove(i);
            self.release(id);
            out.emit(Event::Cancelled {
                req,
                id: Some(id),
                qty: Qty(o.remaining),
                reason: CancelReason::ReduceToZero,
            });
        } else if total > current_total {
            out.emit(Event::Rejected {
                req,
                reason: RejectReason::InvalidChange,
            });
        } else {
            let o = &mut self.orders[i];
            o.remaining = total_qty.0 - o.filled;
            out.emit(Event::Reduced {
                req,
                id,
                filled: Qty(o.filled),
                remaining: Qty(o.remaining),
            });
        }
    }

    fn replace(
        &mut self,
        env: &Envelope,
        id: OrderId,
        price: Price,
        total_qty: Qty,
        post_only: bool,
        out: &mut impl EventSink,
    ) {
        let req = env.req;
        let reject = |reason| Event::Rejected { req, reason };
        if !self.cfg.price_ok(price) {
            return out.emit(reject(RejectReason::InvalidPrice));
        }
        if total_qty > self.cfg.max_order_qty {
            return out.emit(reject(RejectReason::InvalidQty));
        }
        let Some(i) = self.lookup(id, env.account) else {
            return out.emit(reject(RejectReason::UnknownOrStaleId));
        };
        let old = self.orders[i];

        let new_remaining = total_qty.0.saturating_sub(old.filled);
        let cancel_old = Event::Cancelled {
            req,
            id: Some(id),
            qty: Qty(old.remaining),
            reason: CancelReason::Replaced,
        };
        if new_remaining == 0 {
            self.orders.remove(i);
            self.release(id);
            return out.emit(cancel_old);
        }

        if post_only && self.marketable(old.side, price) {
            return out.emit(reject(RejectReason::PostOnlyWouldCross));
        }
        if self.free.is_empty() && self.will_retire(id) {
            return out.emit(reject(RejectReason::BookFull));
        }

        self.orders.remove(i);
        self.release(id);
        out.emit(cancel_old);
        let kind = if post_only {
            OrderKind::PostOnly { price }
        } else {
            OrderKind::Limit {
                price,
                tif: Tif::Gtc,
            }
        };
        self.submit(env, old.side, new_remaining, kind, out);
    }

    pub fn check_state(&self) -> Result<(), String> {
        let cap = self.cfg.capacity as usize;
        let mut seen = vec![false; cap];
        for o in &self.orders {
            let slot = o.id.slot() as usize;
            if slot >= cap || seen[slot] {
                return Err(format!("S2: bad or duplicate slot {slot}"));
            }
            seen[slot] = true;
            let g = self.generations[slot];
            if g.is_multiple_of(2) || g != o.id.generation() {
                return Err(format!("S2: live order {:?} but slot generation {g}", o.id));
            }
            if !self.cfg.price_ok(o.price) {
                return Err(format!("N2: price {:?} out of range", o.price));
            }
            if o.remaining == 0
                || o.filled as u64 + o.remaining as u64 > self.cfg.max_order_qty.0 as u64
            {
                return Err(format!("N1: bad quantities on {:?}", o));
            }
        }
        for &slot in &self.free {
            let slot = slot as usize;
            if seen[slot] || !self.generations[slot].is_multiple_of(2) {
                return Err(format!("S3: free slot {slot} is live or duplicated"));
            }
            seen[slot] = true;
        }
        if let Some(g) = self
            .generations
            .iter()
            .find(|&&g| g > self.cfg.generation_limit)
        {
            return Err(format!(
                "S4: generation {g} exceeds the limit (should have retired)"
            ));
        }
        if self.orders.len() + self.free.len() + self.retired as usize != cap {
            return Err("S1: live + free + retired != capacity".into());
        }
        if let (Some(b), Some(a)) = (self.best(Side::Buy), self.best(Side::Sell))
            && self.orders[b].price >= self.orders[a].price
        {
            return Err("L6: book is crossed".into());
        }
        Ok(())
    }
}

impl Engine for Reference {
    fn process<S: EventSink>(&mut self, env: &Envelope, out: &mut S) {
        match env.cmd {
            Command::New { side, qty, kind } => self.new_order(env, side, qty, kind, out),
            Command::Cancel { id } => self.cancel(env, id, out),
            Command::Reduce { id, total_qty } => self.reduce(env, id, total_qty, out),
            Command::Replace {
                id,
                price,
                total_qty,
                post_only,
            } => self.replace(env, id, price, total_qty, post_only, out),
        }
    }
}
