use crate::types::*;

const NIL: u32 = u32::MAX;
const WORDS: usize = 2;
pub const MAX_TICKS: u16 = (WORDS * 64) as u16;

const BUY: u8 = 0;
const SELL: u8 = 1;

#[inline]
fn side_code(side: Side) -> u8 {
    match side {
        Side::Buy => BUY,
        Side::Sell => SELL,
    }
}

#[inline]
fn side_of(code: u8) -> Side {
    if code == BUY { Side::Buy } else { Side::Sell }
}

#[repr(C, align(32))]
#[derive(Clone, Copy, Debug)]
struct Slot {
    generation: u32,
    prev: u32,
    next: u32,
    remaining: u32,
    filled: u32,
    account: u32,
    price: u16,
    side: u8,
    _pad: u8,
}

const _: () = assert!(std::mem::size_of::<Slot>() == 32);

impl Slot {
    const EMPTY: Slot = Slot {
        generation: 0,
        prev: NIL,
        next: NIL,
        remaining: 0,
        filled: 0,
        account: 0,
        price: 0,
        side: BUY,
        _pad: 0,
    };
}

#[derive(Clone, Copy, Debug)]
struct Level {
    head: u32,
    tail: u32,
    total: u64,
}

impl Level {
    const EMPTY: Level = Level {
        head: NIL,
        tail: NIL,
        total: 0,
    };
}

struct BookSide {
    levels: Box<[Level]>,
    bits: [u64; WORDS],
}

impl BookSide {
    fn new() -> Self {
        BookSide {
            levels: vec![Level::EMPTY; MAX_TICKS as usize].into_boxed_slice(),
            bits: [0; WORDS],
        }
    }

    #[inline]
    fn set(&mut self, p: u16) {
        self.bits[p as usize / 64] |= 1 << (p % 64);
    }

    #[inline]
    fn clear(&mut self, p: u16) {
        self.bits[p as usize / 64] &= !(1 << (p % 64));
    }

    #[inline]
    fn highest(&self) -> Option<u16> {
        if self.bits[1] != 0 {
            Some(64 + 63 - self.bits[1].leading_zeros() as u16)
        } else if self.bits[0] != 0 {
            Some(63 - self.bits[0].leading_zeros() as u16)
        } else {
            None
        }
    }

    #[inline]
    fn lowest(&self) -> Option<u16> {
        if self.bits[0] != 0 {
            Some(self.bits[0].trailing_zeros() as u16)
        } else if self.bits[1] != 0 {
            Some(64 + self.bits[1].trailing_zeros() as u16)
        } else {
            None
        }
    }
}

#[inline]
fn level_push(book: &mut BookSide, slots: &mut [Slot], p: u16, s: u32) {
    let tail = book.levels[p as usize].tail;
    let slot = &mut slots[s as usize];
    slot.prev = tail;
    slot.next = NIL;
    let remaining = slot.remaining as u64;
    if tail == NIL {
        book.levels[p as usize].head = s;
        book.set(p);
    } else {
        slots[tail as usize].next = s;
    }
    let level = &mut book.levels[p as usize];
    level.tail = s;
    level.total += remaining;
}

#[inline]
fn level_remove(book: &mut BookSide, slots: &mut [Slot], p: u16, s: u32) {
    let Slot {
        prev,
        next,
        remaining,
        ..
    } = slots[s as usize];
    if prev == NIL {
        book.levels[p as usize].head = next;
    } else {
        slots[prev as usize].next = next;
    }
    if next == NIL {
        book.levels[p as usize].tail = prev;
    } else {
        slots[next as usize].prev = prev;
    }
    let slot = &mut slots[s as usize];
    slot.prev = NIL;
    slot.next = NIL;
    let level = &mut book.levels[p as usize];
    assert!(level.total >= remaining as u64, "level total underflow");
    level.total -= remaining as u64;
    if level.head == NIL {
        book.clear(p);
    }
}

#[inline]
fn level_sub_qty(book: &mut BookSide, p: u16, qty: u32) {
    let level = &mut book.levels[p as usize];
    assert!(level.total >= qty as u64, "level total underflow");
    level.total -= qty as u64;
}

pub struct FastBook {
    cfg: Config,
    slots: Box<[Slot]>,
    free: Vec<u32>,
    retired: u32,
    live: u32,
    bids: BookSide,
    asks: BookSide,
}

impl FastBook {
    pub fn new(cfg: Config) -> Self {
        cfg.validate().expect("invalid config");
        assert!(
            cfg.max_tick.0 < MAX_TICKS,
            "FastBook supports prices below {MAX_TICKS}"
        );
        FastBook {
            cfg,
            slots: vec![Slot::EMPTY; cfg.capacity as usize].into_boxed_slice(),
            free: (0..cfg.capacity).rev().collect(),
            retired: 0,
            live: 0,
            bids: BookSide::new(),
            asks: BookSide::new(),
        }
    }

    pub fn best_bid(&self) -> Option<Price> {
        self.bids.highest().map(Price)
    }

    pub fn best_ask(&self) -> Option<Price> {
        self.asks.lowest().map(Price)
    }

    pub fn live_orders(&self) -> usize {
        self.live as usize
    }

    pub fn retired_slots(&self) -> u32 {
        self.retired
    }

    #[inline]
    fn side_mut(&mut self, side: Side) -> (&mut BookSide, &mut [Slot]) {
        match side {
            Side::Buy => (&mut self.bids, &mut self.slots),
            Side::Sell => (&mut self.asks, &mut self.slots),
        }
    }

    #[inline]
    fn best(&self, side: Side) -> Option<u16> {
        match side {
            Side::Buy => self.bids.highest(),
            Side::Sell => self.asks.lowest(),
        }
    }

    #[inline]
    fn marketable(&self, side: Side, limit: Price) -> bool {
        self.best(side.opposite())
            .is_some_and(|p| side.crosses(limit, Price(p)))
    }

    #[inline]
    fn alloc(&mut self) -> Option<u32> {
        let s = self.free.pop()?;
        self.slots[s as usize].generation += 1;
        self.live += 1;
        Some(s)
    }

    #[inline]
    fn will_retire(&self, s: u32) -> bool {
        self.slots[s as usize].generation as u64 + 2 > self.cfg.generation_limit as u64
    }

    #[inline]
    fn release(&mut self, s: u32) {
        self.live -= 1;
        if self.will_retire(s) {
            self.slots[s as usize].generation -= 1;
            self.retired += 1;
        } else {
            self.slots[s as usize].generation += 1;
            self.free.push(s);
        }
    }

    #[inline]
    fn id_of(&self, s: u32) -> OrderId {
        OrderId::new(s, self.slots[s as usize].generation)
    }

    #[inline]
    fn lookup(&self, id: OrderId, account: AccountId) -> Option<u32> {
        let s = id.slot();
        let slot = self.slots.get(s as usize)?;
        let live = slot.generation & 1 == 1;
        (live && slot.generation == id.generation() && slot.account == account).then_some(s)
    }

    #[inline]
    fn remove(&mut self, s: u32) {
        let slot = self.slots[s as usize];
        let (book, slots) = self.side_mut(side_of(slot.side));
        level_remove(book, slots, slot.price, s);
        self.release(s);
    }

    fn new_order(
        &mut self,
        env: &Envelope,
        side: Side,
        qty: Qty,
        kind: OrderKind,
        out: &mut impl EventSink,
    ) {
        let price = match kind {
            OrderKind::Limit { price, .. } | OrderKind::PostOnly { price } => Some(price),
            OrderKind::Market { protection } => protection,
        };
        if price.is_some_and(|p| !self.cfg.price_ok(p)) {
            return out.emit(Event::Rejected {
                req: env.req,
                reason: RejectReason::InvalidPrice,
            });
        }
        if !self.cfg.qty_ok(qty) {
            return out.emit(Event::Rejected {
                req: env.req,
                reason: RejectReason::InvalidQty,
            });
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

        let maker_side = side.opposite();
        let mut remaining = qty;
        while remaining > 0 {
            let Some(p) = self.best(maker_side) else {
                break;
            };
            if !side.crosses(limit, Price(p)) {
                break;
            }
            let (book, slots) = self.side_mut(maker_side);
            let s = book.levels[p as usize].head;
            let maker = &mut slots[s as usize];
            if maker.account == env.account {
                return out.emit(Event::Cancelled {
                    req,
                    id: None,
                    qty: Qty(remaining),
                    reason: CancelReason::SelfTrade,
                });
            }
            let fill = remaining.min(maker.remaining);
            remaining -= fill;
            maker.remaining -= fill;
            maker.filled += fill;
            let maker_remaining = maker.remaining;
            let maker_id = OrderId::new(s, maker.generation);
            level_sub_qty(book, p, fill);
            out.emit(Event::Trade {
                taker_req: req,
                taker_side: side,
                maker_id,
                price: Price(p),
                qty: Qty(fill),
                maker_remaining: Qty(maker_remaining),
                ts: env.ts,
            });
            if maker_remaining == 0 {
                level_remove(book, slots, p, s);
                self.release(s);
                out.emit(Event::MakerFilled { id: maker_id });
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
        } else if let Some(s) = self.alloc() {
            let slot = &mut self.slots[s as usize];
            slot.remaining = remaining;
            slot.filled = 0;
            slot.account = env.account;
            slot.price = limit.0;
            slot.side = side_code(side);
            let (book, slots) = self.side_mut(side);
            level_push(book, slots, limit.0, s);
            out.emit(Event::Rested {
                req,
                id: self.id_of(s),
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
        let Some(s) = self.lookup(id, env.account) else {
            return out.emit(Event::Rejected {
                req: env.req,
                reason: RejectReason::UnknownOrStaleId,
            });
        };
        let remaining = self.slots[s as usize].remaining;
        self.remove(s);
        out.emit(Event::Cancelled {
            req: env.req,
            id: Some(id),
            qty: Qty(remaining),
            reason: CancelReason::User,
        });
    }

    fn reduce(&mut self, env: &Envelope, id: OrderId, total_qty: Qty, out: &mut impl EventSink) {
        let req = env.req;
        let Some(s) = self.lookup(id, env.account) else {
            return out.emit(Event::Rejected {
                req,
                reason: RejectReason::UnknownOrStaleId,
            });
        };
        let Slot {
            filled,
            remaining,
            price,
            side,
            ..
        } = self.slots[s as usize];
        let total = total_qty.0 as u64;
        if total <= filled as u64 {
            self.remove(s);
            out.emit(Event::Cancelled {
                req,
                id: Some(id),
                qty: Qty(remaining),
                reason: CancelReason::ReduceToZero,
            });
        } else if total > filled as u64 + remaining as u64 {
            out.emit(Event::Rejected {
                req,
                reason: RejectReason::InvalidChange,
            });
        } else {
            let new_remaining = total_qty.0 - filled;
            let (book, slots) = self.side_mut(side_of(side));
            level_sub_qty(book, price, remaining - new_remaining);
            slots[s as usize].remaining = new_remaining;
            out.emit(Event::Reduced {
                req,
                id,
                filled: Qty(filled),
                remaining: Qty(new_remaining),
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
        if !self.cfg.price_ok(price) {
            return out.emit(Event::Rejected {
                req,
                reason: RejectReason::InvalidPrice,
            });
        }
        if total_qty > self.cfg.max_order_qty {
            return out.emit(Event::Rejected {
                req,
                reason: RejectReason::InvalidQty,
            });
        }
        let Some(s) = self.lookup(id, env.account) else {
            return out.emit(Event::Rejected {
                req,
                reason: RejectReason::UnknownOrStaleId,
            });
        };
        let old = self.slots[s as usize];
        let side = side_of(old.side);
        let cancel_old = Event::Cancelled {
            req,
            id: Some(id),
            qty: Qty(old.remaining),
            reason: CancelReason::Replaced,
        };
        let new_remaining = total_qty.0.saturating_sub(old.filled);
        if new_remaining == 0 {
            self.remove(s);
            return out.emit(cancel_old);
        }
        if post_only && self.marketable(side, price) {
            return out.emit(Event::Rejected {
                req,
                reason: RejectReason::PostOnlyWouldCross,
            });
        }
        if self.free.is_empty() && self.will_retire(s) {
            return out.emit(Event::Rejected {
                req,
                reason: RejectReason::BookFull,
            });
        }
        self.remove(s);
        out.emit(cancel_old);
        let kind = if post_only {
            OrderKind::PostOnly { price }
        } else {
            OrderKind::Limit {
                price,
                tif: Tif::Gtc,
            }
        };
        self.submit(env, side, new_remaining, kind, out);
    }

    pub fn check_invariants(&self) -> Result<(), String> {
        let cap = self.cfg.capacity as usize;
        let mut seen = vec![false; cap];
        let mut linked = 0usize;
        for (side, book) in [(Side::Buy, &self.bids), (Side::Sell, &self.asks)] {
            for p in 0..MAX_TICKS {
                let level = book.levels[p as usize];
                let bit = book.bits[p as usize / 64] >> (p % 64) & 1 == 1;
                let empty = level.head == NIL;
                if empty != (level.tail == NIL) || empty != (level.total == 0) || empty == bit {
                    return Err(format!(
                        "L1: {side:?} level {p}: head/tail/total/bit disagree: {level:?} bit={bit}"
                    ));
                }
                if !empty && !self.cfg.price_ok(Price(p)) {
                    return Err(format!("N2: {side:?} level {p} is outside the tick range"));
                }
                let (mut s, mut prev, mut sum) = (level.head, NIL, 0u64);
                while s != NIL {
                    let slot = self
                        .slots
                        .get(s as usize)
                        .ok_or(format!("L3: link to slot {s} out of range"))?;
                    if seen[s as usize] {
                        return Err(format!("L3: slot {s} linked twice (cycle or shared)"));
                    }
                    seen[s as usize] = true;
                    linked += 1;
                    if slot.prev != prev {
                        return Err(format!(
                            "L3: slot {s} prev is {}, expected {prev}",
                            slot.prev
                        ));
                    }
                    if slot.generation & 1 == 0 {
                        return Err(format!("S2: slot {s} is linked but not live"));
                    }
                    if side_of(slot.side) != side || slot.price != p {
                        return Err(format!(
                            "L4: slot {s} is in {side:?}@{p} but says {:?}@{}",
                            side_of(slot.side),
                            slot.price
                        ));
                    }
                    if slot.remaining == 0
                        || slot.filled as u64 + slot.remaining as u64
                            > self.cfg.max_order_qty.0 as u64
                    {
                        return Err(format!("N1: slot {s} has bad quantities {slot:?}"));
                    }
                    sum += slot.remaining as u64;
                    prev = s;
                    s = slot.next;
                }
                if prev != level.tail {
                    return Err(format!(
                        "L3: {side:?} level {p} tail is {}, walk ended at {prev}",
                        level.tail
                    ));
                }
                if sum != level.total {
                    return Err(format!(
                        "L2: {side:?} level {p} total {} != sum {sum}",
                        level.total
                    ));
                }
            }
        }
        if linked != self.live as usize {
            return Err(format!(
                "S2: {linked} linked slots but live count {}",
                self.live
            ));
        }
        for &s in &self.free {
            let slot = &self.slots[s as usize];
            if seen[s as usize] || slot.generation & 1 == 1 || slot.prev != NIL || slot.next != NIL
            {
                return Err(format!(
                    "S3: free slot {s} is live, duplicated or still linked"
                ));
            }
            seen[s as usize] = true;
        }
        let unaccounted = seen.iter().filter(|&&v| !v).count();
        if unaccounted != self.retired as usize {
            return Err(format!(
                "S1: {unaccounted} slots neither live nor free, but {} retired",
                self.retired
            ));
        }
        if let Some(slot) = self
            .slots
            .iter()
            .find(|s| s.generation > self.cfg.generation_limit)
        {
            return Err(format!(
                "S4: generation {} exceeds the limit",
                slot.generation
            ));
        }
        if let (Some(b), Some(a)) = (self.best_bid(), self.best_ask())
            && b >= a
        {
            return Err(format!("L6: book crossed: bid {b:?} >= ask {a:?}"));
        }
        Ok(())
    }
}

impl Engine for FastBook {
    #[inline]
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
