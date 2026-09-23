use std::collections::HashMap;

use crate::types::*;

pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed.max(1))
    }

    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    #[inline]
    pub fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n
    }

    #[inline]
    pub fn range(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.below(hi - lo + 1)
    }

    #[inline]
    pub fn geometric(&mut self, max: u32) -> u32 {
        self.next_u64().trailing_zeros().min(max)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Op {
    GtcRest,
    GtcMatch,
    Ioc,
    Market,
    PostOnlyRest,
    PostOnlyReject,
    Cancel,
    Reduce,
    Replace,
    OtherReject,
}

impl Op {
    pub const ALL: [Op; 10] = [
        Op::GtcRest,
        Op::GtcMatch,
        Op::Ioc,
        Op::Market,
        Op::PostOnlyRest,
        Op::PostOnlyReject,
        Op::Cancel,
        Op::Reduce,
        Op::Replace,
        Op::OtherReject,
    ];

    pub fn classify(env: &Envelope, events: &[Event]) -> Op {
        let traded = events.iter().any(|e| matches!(e, Event::Trade { .. }));
        match env.cmd {
            Command::New { kind, .. } => match kind {
                OrderKind::PostOnly { .. } if traded => unreachable!("post-only traded"),
                OrderKind::PostOnly { .. } => match events {
                    [Event::Rested { .. }] => Op::PostOnlyRest,
                    _ => Op::PostOnlyReject,
                },
                _ if matches!(events, [Event::Rejected { .. }]) => Op::OtherReject,
                OrderKind::Limit { tif: Tif::Gtc, .. } if traded => Op::GtcMatch,
                OrderKind::Limit { tif: Tif::Gtc, .. } => Op::GtcRest,
                OrderKind::Limit { tif: Tif::Ioc, .. } => Op::Ioc,
                OrderKind::Market { .. } => Op::Market,
            },
            Command::Cancel { .. } => Op::Cancel,
            Command::Reduce { .. } => Op::Reduce,
            Command::Replace { .. } => Op::Replace,
        }
    }
}

#[derive(Clone, Copy)]
struct LiveOrder {
    id: OrderId,
    account: AccountId,
    side: Side,
    qty: u32,
}

pub struct Workload {
    rng: Rng,
    live: Vec<LiveOrder>,
    index: HashMap<OrderId, usize>,
    req: u64,
    pub target_live: usize,
    pub accounts: u32,
}

impl Workload {
    pub fn new(seed: u64, target_live: usize) -> Self {
        Workload {
            rng: Rng::new(seed),
            live: Vec::new(),
            index: HashMap::new(),
            req: 0,
            target_live,
            accounts: 64,
        }
    }

    pub fn live_orders(&self) -> usize {
        self.live.len()
    }

    pub fn next(&mut self, best_bid: Option<Price>, best_ask: Option<Price>) -> Envelope {
        self.req += 1;
        let bid = best_bid.map_or(49, |p| p.0 as i64);
        let ask = best_ask.map_or(51, |p| p.0 as i64);
        let clamp = |p: i64| Price(p.clamp(1, 99) as u16);
        let r = self.rng.below(100);
        let crowded = self.live.len() > self.target_live;

        if !self.live.is_empty() && (r < 22 || (crowded && r < 60)) {
            let o = self.pick();
            return self.envelope(o.account, Command::Cancel { id: o.id });
        }
        if !self.live.is_empty() && r < 27 {
            let o = self.pick();
            let total_qty = Qty(self.rng.range(1, o.qty as u64) as u32);
            return self.envelope(
                o.account,
                Command::Reduce {
                    id: o.id,
                    total_qty,
                },
            );
        }
        if !self.live.is_empty() && r < 32 {
            let o = self.pick();
            let depth = self.rng.geometric(8) as i64;
            let price = match o.side {
                Side::Buy => clamp(ask - 1 - depth),
                Side::Sell => clamp(bid + 1 + depth),
            };
            let total_qty = Qty(self.rng.range(1, 20) as u32);
            let post_only = self.rng.below(2) == 0;
            return self.envelope(
                o.account,
                Command::Replace {
                    id: o.id,
                    price,
                    total_qty,
                    post_only,
                },
            );
        }

        let side = if self.rng.below(2) == 0 {
            Side::Buy
        } else {
            Side::Sell
        };
        let account = self.rng.below(self.accounts as u64) as u32;
        let depth = self.rng.geometric(8) as i64;
        let passive = match side {
            Side::Buy => clamp(ask - 1 - depth),
            Side::Sell => clamp(bid + 1 + depth),
        };
        let aggressive = match side {
            Side::Buy => clamp(ask + self.rng.below(3) as i64),
            Side::Sell => clamp(bid - self.rng.below(3) as i64),
        };
        let r = self.rng.below(100);
        let (qty, kind) = if r < 55 {
            (
                self.rng.range(1, 20),
                OrderKind::Limit {
                    price: passive,
                    tif: Tif::Gtc,
                },
            )
        } else if r < 70 {
            (
                self.rng.range(1, 40),
                OrderKind::Limit {
                    price: aggressive,
                    tif: Tif::Gtc,
                },
            )
        } else if r < 78 {
            (
                self.rng.range(1, 40),
                OrderKind::Limit {
                    price: aggressive,
                    tif: Tif::Ioc,
                },
            )
        } else if r < 83 {
            let protection = (self.rng.below(2) == 0).then_some(aggressive);
            (self.rng.range(1, 40), OrderKind::Market { protection })
        } else {
            let price = if self.rng.below(5) == 0 {
                aggressive
            } else {
                passive
            };
            (self.rng.range(1, 20), OrderKind::PostOnly { price })
        };
        self.envelope(
            account,
            Command::New {
                side,
                qty: Qty(qty as u32),
                kind,
            },
        )
    }

    pub fn observe(&mut self, env: &Envelope, events: &[Event]) {
        let side = match env.cmd {
            Command::New { side, .. } => Some(side),
            Command::Replace { id, .. } => self.index.get(&id).map(|&i| self.live[i].side),
            _ => None,
        };
        for e in events {
            match *e {
                Event::Rested { id, qty, .. } => {
                    let side = side.expect("only New and Replace of a tracked order rest");
                    self.index.insert(id, self.live.len());
                    self.live.push(LiveOrder {
                        id,
                        account: env.account,
                        side,
                        qty: qty.0,
                    });
                }
                Event::MakerFilled { id } | Event::Cancelled { id: Some(id), .. } => {
                    self.forget(id)
                }
                _ => {}
            }
        }
    }

    fn envelope(&self, account: AccountId, cmd: Command) -> Envelope {
        Envelope {
            req: self.req,
            ts: self.req * 10,
            account,
            cmd,
        }
    }

    fn pick(&mut self) -> LiveOrder {
        self.live[self.rng.below(self.live.len() as u64) as usize]
    }

    fn forget(&mut self, id: OrderId) {
        if let Some(i) = self.index.remove(&id) {
            self.live.swap_remove(i);
            if let Some(moved) = self.live.get(i) {
                self.index.insert(moved.id, i);
            }
        }
    }
}
