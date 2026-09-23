#![allow(dead_code)]

use omnibook::types::*;
use proptest::prelude::*;

#[derive(Clone, Debug)]
pub enum Pick {
    Issued(usize),
    Raw(u64),
}

#[derive(Clone, Debug)]
pub enum Kind {
    Gtc(u16),
    Ioc(u16),
    Market(Option<u16>),
    PostOnly(u16),
}

#[derive(Clone, Debug)]
pub enum Cmd {
    New(Side, u32, Kind),
    Cancel(Pick),
    Reduce(Pick, u32),
    Replace(Pick, u16, u32, bool),
}

#[derive(Clone, Debug)]
pub struct Step {
    pub account: AccountId,
    pub cmd: Cmd,
}

pub struct Profile {
    pub cfg: Config,
    pub prices: (u16, u16),
}

pub fn tiny() -> Profile {
    Profile {
        cfg: Config {
            min_tick: Price(1),
            max_tick: Price(5),
            max_order_qty: Qty(10),
            capacity: 4,
            generation_limit: 3,
        },
        prices: (1, 5),
    }
}

pub fn cents() -> Profile {
    Profile {
        cfg: Config {
            max_order_qty: Qty(10),
            ..Config::binary_cents(64)
        },
        prices: (46, 54),
    }
}

pub fn step(p: &Profile) -> impl Strategy<Value = Step> + use<> {
    let (lo, hi) = p.prices;
    let max_qty = p.cfg.max_order_qty.0;
    let bad_hi = p.cfg.max_tick.0 + 1;
    let price = prop_oneof![20 => lo..=hi, 1 => Just(0u16), 1 => Just(bad_hi)];
    let qty = prop_oneof![20 => 1..=5u32, 1 => Just(0u32), 1 => Just(max_qty + 1)];
    let total = 0..=max_qty + 2;
    let side = prop_oneof![Just(Side::Buy), Just(Side::Sell)];
    let pick = prop_oneof![
        9 => any::<usize>().prop_map(Pick::Issued),
        1 => any::<u64>().prop_map(Pick::Raw),
    ];
    let kind = prop_oneof![
        4 => price.clone().prop_map(Kind::Gtc),
        2 => price.clone().prop_map(Kind::Ioc),
        1 => proptest::option::of(price.clone()).prop_map(Kind::Market),
        2 => price.clone().prop_map(Kind::PostOnly),
    ];
    let cmd = prop_oneof![
        6 => (side, qty, kind).prop_map(|(s, q, k)| Cmd::New(s, q, k)),
        2 => pick.clone().prop_map(Cmd::Cancel),
        1 => (pick.clone(), total.clone()).prop_map(|(p, t)| Cmd::Reduce(p, t)),
        1 => (pick, price, total, any::<bool>())
            .prop_map(|(p, px, t, po)| Cmd::Replace(p, px, t, po)),
    ];
    (0..3u32, cmd).prop_map(|(account, cmd)| Step { account, cmd })
}

fn resolve(pick: &Pick, issued: &[OrderId]) -> OrderId {
    match pick {
        Pick::Issued(i) if !issued.is_empty() => issued[i % issued.len()],
        Pick::Issued(_) => OrderId(0),
        Pick::Raw(raw) => OrderId(*raw),
    }
}

pub fn envelope(n: usize, step: &Step, issued: &[OrderId]) -> Envelope {
    let cmd = match &step.cmd {
        Cmd::New(side, qty, kind) => Command::New {
            side: *side,
            qty: Qty(*qty),
            kind: match *kind {
                Kind::Gtc(p) => OrderKind::Limit {
                    price: Price(p),
                    tif: Tif::Gtc,
                },
                Kind::Ioc(p) => OrderKind::Limit {
                    price: Price(p),
                    tif: Tif::Ioc,
                },
                Kind::Market(p) => OrderKind::Market {
                    protection: p.map(Price),
                },
                Kind::PostOnly(p) => OrderKind::PostOnly { price: Price(p) },
            },
        },
        Cmd::Cancel(p) => Command::Cancel {
            id: resolve(p, issued),
        },
        Cmd::Reduce(p, t) => Command::Reduce {
            id: resolve(p, issued),
            total_qty: Qty(*t),
        },
        Cmd::Replace(p, px, t, po) => Command::Replace {
            id: resolve(p, issued),
            price: Price(*px),
            total_qty: Qty(*t),
            post_only: *po,
        },
    };
    Envelope {
        req: n as u64,
        ts: 1_000 + n as u64,
        account: step.account,
        cmd,
    }
}

pub fn record_issued(issued: &mut Vec<OrderId>, events: &[Event]) {
    issued.extend(events.iter().filter_map(|e| match e {
        Event::Rested { id, .. } => Some(*id),
        _ => None,
    }));
}
