#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Price(pub u16);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Qty(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OrderId(pub u64);

impl OrderId {
    #[inline]
    pub const fn new(slot: u32, generation: u32) -> Self {
        OrderId(((generation as u64) << 32) | slot as u64)
    }
    #[inline]
    pub const fn slot(self) -> u32 {
        self.0 as u32
    }
    #[inline]
    pub const fn generation(self) -> u32 {
        (self.0 >> 32) as u32
    }
}

pub type AccountId = u32;
pub type ReqId = u64;
pub type Timestamp = u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Side {
    Buy,
    Sell,
}

impl Side {
    #[inline]
    pub fn opposite(self) -> Side {
        match self {
            Side::Buy => Side::Sell,
            Side::Sell => Side::Buy,
        }
    }

    #[inline]
    pub fn crosses(self, limit: Price, resting: Price) -> bool {
        match self {
            Side::Buy => resting <= limit,
            Side::Sell => resting >= limit,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tif {
    Gtc,
    Ioc,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrderKind {
    Limit { price: Price, tif: Tif },
    Market { protection: Option<Price> },
    PostOnly { price: Price },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    New {
        side: Side,
        qty: Qty,
        kind: OrderKind,
    },
    Cancel {
        id: OrderId,
    },
    Reduce {
        id: OrderId,
        total_qty: Qty,
    },
    Replace {
        id: OrderId,
        price: Price,
        total_qty: Qty,
        post_only: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Envelope {
    pub req: ReqId,
    pub ts: Timestamp,
    pub account: AccountId,
    pub cmd: Command,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RejectReason {
    InvalidPrice,
    InvalidQty,
    PostOnlyWouldCross,
    BookFull,
    UnknownOrStaleId,
    InvalidChange,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CancelReason {
    User,
    Unfilled,
    SelfTrade,
    ReduceToZero,
    Replaced,
    CapacityExhausted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Rejected {
        req: ReqId,
        reason: RejectReason,
    },
    Trade {
        taker_req: ReqId,
        taker_side: Side,
        maker_id: OrderId,
        price: Price,
        qty: Qty,
        maker_remaining: Qty,
        ts: Timestamp,
    },
    MakerFilled {
        id: OrderId,
    },
    Rested {
        req: ReqId,
        id: OrderId,
        qty: Qty,
    },
    Filled {
        req: ReqId,
    },
    Cancelled {
        req: ReqId,
        id: Option<OrderId>,
        qty: Qty,
        reason: CancelReason,
    },
    Reduced {
        req: ReqId,
        id: OrderId,
        filled: Qty,
        remaining: Qty,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    pub min_tick: Price,
    pub max_tick: Price,
    pub max_order_qty: Qty,
    pub capacity: u32,
    pub generation_limit: u32,
}

impl Config {
    pub fn binary_cents(capacity: u32) -> Self {
        Config {
            min_tick: Price(1),
            max_tick: Price(99),
            max_order_qty: Qty(1_000_000),
            capacity,
            generation_limit: u32::MAX,
        }
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        if self.min_tick.0 == 0 {
            return Err("min_tick must be >= 1");
        }
        if self.min_tick > self.max_tick {
            return Err("min_tick must be <= max_tick");
        }
        if self.max_order_qty.0 == 0 {
            return Err("max_order_qty must be >= 1");
        }
        if self.capacity == 0 || self.capacity == u32::MAX {
            return Err("capacity must be in 1..u32::MAX (u32::MAX is the NIL index)");
        }
        if self.generation_limit == 0 {
            return Err("generation_limit must be >= 1");
        }
        Ok(())
    }

    #[inline]
    pub fn price_ok(&self, p: Price) -> bool {
        self.min_tick <= p && p <= self.max_tick
    }

    #[inline]
    pub fn qty_ok(&self, q: Qty) -> bool {
        1 <= q.0 && q <= self.max_order_qty
    }

    #[inline]
    pub fn limit_of(&self, side: Side, kind: OrderKind) -> Price {
        match kind {
            OrderKind::Limit { price, .. } | OrderKind::PostOnly { price } => price,
            OrderKind::Market { protection } => protection.unwrap_or(match side {
                Side::Buy => self.max_tick,
                Side::Sell => self.min_tick,
            }),
        }
    }
}

#[inline]
pub fn rests(kind: OrderKind) -> bool {
    match kind {
        OrderKind::Limit { tif, .. } => tif == Tif::Gtc,
        OrderKind::PostOnly { .. } => true,
        OrderKind::Market { .. } => false,
    }
}

pub trait EventSink {
    fn emit(&mut self, event: Event);
}

impl EventSink for Vec<Event> {
    #[inline]
    fn emit(&mut self, event: Event) {
        self.push(event);
    }
}

pub trait Engine {
    fn process<S: EventSink>(&mut self, env: &Envelope, out: &mut S);
}
