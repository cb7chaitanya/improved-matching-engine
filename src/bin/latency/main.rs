mod legacy;

use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::{BTreeMap, HashMap};
use std::hint::black_box;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::time::Instant;

use hdrhistogram::Histogram;
use omnibook::book::FastBook;
use omnibook::types::*;
use omnibook::workload::{Op, Rng, Workload};

struct Counting;
static ALLOCS: AtomicU64 = AtomicU64::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Relaxed);
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        ALLOCS.fetch_add(1, Relaxed);
        unsafe { System.realloc(p, l, n) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn prefer_performance_cores() {
    #[cfg(target_os = "macos")]
    {
        const QOS_CLASS_USER_INTERACTIVE: u32 = 0x21;
        unsafe extern "C" {
            fn pthread_set_qos_class_self_np(qos: u32, relative_priority: i32) -> i32;
        }
        unsafe {
            pthread_set_qos_class_self_np(QOS_CLASS_USER_INTERACTIVE, 0);
        }
    }
}

struct Stats {
    hist: Histogram<u64>,
    allocs: u64,
}

impl Stats {
    fn new() -> Self {
        Stats {
            hist: Histogram::new_with_bounds(1, 60_000_000_000, 3).unwrap(),
            allocs: 0,
        }
    }
}

fn header(title: &str) {
    println!("\n{title}");
    println!(
        "{:<16}{:>10}{:>9}{:>9}{:>9}{:>10}{:>10}{:>11}{:>11}",
        "op", "count", "p50", "p90", "p99", "p99.9", "p99.99", "max", "allocs/op"
    );
}

fn ns(v: u64) -> String {
    if v <= 1 {
        "<42".to_string()
    } else {
        v.to_string()
    }
}

fn row(name: &str, s: &Stats) {
    let h = &s.hist;
    let q = |x| ns(h.value_at_quantile(x));
    println!(
        "{:<16}{:>10}{:>9}{:>9}{:>9}{:>10}{:>10}{:>11}{:>11.2}",
        name,
        h.len(),
        q(0.50),
        q(0.90),
        q(0.99),
        q(0.999),
        q(0.9999),
        ns(h.max()),
        s.allocs as f64 / h.len().max(1) as f64
    );
}

fn record(cfg: Config, ops: usize, seed: u64, target_live: usize) -> Vec<(Envelope, Op)> {
    let mut engine = FastBook::new(cfg);
    let mut flow = Workload::new(seed, target_live);
    let mut out = Vec::with_capacity(4096);
    let mut stream = Vec::with_capacity(ops);
    for _ in 0..ops {
        let env = flow.next(engine.best_bid(), engine.best_ask());
        out.clear();
        engine.process(&env, &mut out);
        flow.observe(&env, &out);
        stream.push((env, Op::classify(&env, &out)));
    }
    stream
}

fn fast_latency(cfg: Config, stream: &[(Envelope, Op)], warmup: usize) -> BTreeMap<Op, Stats> {
    let mut engine = FastBook::new(cfg);
    let mut out = Vec::with_capacity(4096);
    let mut stats: BTreeMap<Op, Stats> = Op::ALL.iter().map(|&op| (op, Stats::new())).collect();
    for (i, (env, op)) in stream.iter().enumerate() {
        out.clear();
        let a0 = ALLOCS.load(Relaxed);
        let t0 = Instant::now();
        engine.process(env, &mut out);
        let ns = t0.elapsed().as_nanos() as u64;
        let allocs = ALLOCS.load(Relaxed) - a0;
        if i >= warmup {
            let s = stats.get_mut(op).unwrap();
            s.hist.record(ns.max(1)).unwrap();
            s.allocs += allocs;
        }
    }
    stats
}

fn fast_throughput(cfg: Config, stream: &[(Envelope, Op)]) -> f64 {
    let mut engine = FastBook::new(cfg);
    let mut out = Vec::with_capacity(4096);
    let t0 = Instant::now();
    for (env, _) in stream {
        out.clear();
        engine.process(env, &mut out);
        black_box(&out);
    }
    t0.elapsed().as_nanos() as f64 / stream.len() as f64
}

fn gtc_flow(n: usize, seed: u64) -> Vec<(Side, u16, u32)> {
    let mut rng = Rng::new(seed);
    (0..n)
        .map(|_| {
            let side = if rng.below(2) == 0 {
                Side::Buy
            } else {
                Side::Sell
            };
            let price = match side {
                Side::Buy => rng.range(38, 52) as u16,
                Side::Sell => rng.range(48, 62) as u16,
            };
            (side, price, rng.range(1, 20) as u32)
        })
        .collect()
}

fn compare_with_legacy(n: usize) {
    let flow = gtc_flow(n, 7);

    let orders: Vec<legacy::models::Order> = flow
        .iter()
        .enumerate()
        .map(|(i, &(side, price, qty))| legacy::models::Order {
            id: i as u64,
            symbol: "BTC/USD".to_string(),
            side: match side {
                Side::Buy => legacy::models::Side::Buy,
                Side::Sell => legacy::models::Side::Sell,
            },
            price: price as u64,
            qty: qty as u64,
            remaining_qty: qty as u64,
            timestamp: i as u64,
        })
        .collect();
    let mut books = HashMap::new();
    let mut old = Stats::new();
    let t_all = Instant::now();
    let mut untimed = 0u128;
    for order in orders {
        let a0 = ALLOCS.load(Relaxed);
        let t0 = Instant::now();
        let result = legacy::process_order(&mut books, order);
        let ns = t0.elapsed().as_nanos() as u64;
        old.allocs += ALLOCS.load(Relaxed) - a0;
        old.hist.record(ns.max(1)).unwrap();
        let t1 = Instant::now();
        drop(black_box(result));
        untimed += t1.elapsed().as_nanos();
    }
    let old_mean = (t_all.elapsed().as_nanos() - untimed) as f64 / n as f64;

    let cfg = Config {
        max_order_qty: Qty(1_000),
        ..Config::binary_cents(n as u32 + 1)
    };
    let envs: Vec<Envelope> = flow
        .iter()
        .enumerate()
        .map(|(i, &(side, price, qty))| Envelope {
            req: i as u64,
            ts: i as u64,
            account: i as u32,
            cmd: Command::New {
                side,
                qty: Qty(qty),
                kind: OrderKind::Limit {
                    price: Price(price),
                    tif: Tif::Gtc,
                },
            },
        })
        .collect();
    let mut engine = FastBook::new(cfg);
    let mut out = Vec::with_capacity(4096);
    let mut new = Stats::new();
    for env in &envs {
        out.clear();
        let a0 = ALLOCS.load(Relaxed);
        let t0 = Instant::now();
        engine.process(env, &mut out);
        let ns = t0.elapsed().as_nanos() as u64;
        new.allocs += ALLOCS.load(Relaxed) - a0;
        new.hist.record(ns.max(1)).unwrap();
    }
    let mut engine = FastBook::new(cfg);
    let t0 = Instant::now();
    for env in &envs {
        out.clear();
        engine.process(env, &mut out);
        black_box(&out);
    }
    let new_mean = t0.elapsed().as_nanos() as f64 / n as f64;

    header(&format!(
        "Like-for-like: {n} GTC limit orders, no cancels (book grows to {} orders)",
        engine.live_orders()
    ));
    row("original", &old);
    row("fast", &new);
    println!(
        "mean ns/op: original {old_mean:.1}, fast {new_mean:.1}  ({:.1}x)",
        old_mean / new_mean
    );
    for (name, q) in [("p50", 0.5), ("p99", 0.99), ("p99.9", 0.999)] {
        let (a, b) = (old.hist.value_at_quantile(q), new.hist.value_at_quantile(q));
        println!("{name:>6}: {} ns -> {} ns", ns(a), ns(b));
    }
}

fn main() {
    prefer_performance_cores();
    let ops: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(2_000_000);
    let warmup = ops / 20;
    let target_live = 5_000;
    let cfg = Config {
        max_order_qty: Qty(1_000),
        ..Config::binary_cents(1 << 16)
    };

    println!("omnibook latency benchmark — service time of FastBook::process, nanoseconds");
    println!("timer resolution on this machine: see note in source (41.67 ns on Apple Silicon)");

    let stream = record(cfg, ops, 1, target_live);

    const RUNS: usize = 5;
    let mut combined: BTreeMap<Op, Stats> = Op::ALL.iter().map(|&op| (op, Stats::new())).collect();
    println!("\nPer-run spread ({RUNS} replays of the same stream, fresh engine each):");
    for run in 1..=RUNS {
        let stats = fast_latency(cfg, &stream, warmup);
        let mut all = Stats::new();
        for (op, s) in &stats {
            all.hist.add(&s.hist).unwrap();
            let c = combined.get_mut(op).unwrap();
            c.hist.add(&s.hist).unwrap();
            c.allocs += s.allocs;
        }
        let q = |x| ns(all.hist.value_at_quantile(x));
        println!(
            "  run {run}: p99={} p99.9={} p99.99={} max={}",
            q(0.99),
            q(0.999),
            q(0.9999),
            ns(all.hist.max())
        );
    }

    header(&format!(
        "Mixed binary-market flow: {ops} commands x {RUNS} runs, ~{target_live} resting orders, first {warmup} of each run excluded"
    ));
    let mut all = Stats::new();
    for (op, s) in &combined {
        if !s.hist.is_empty() {
            row(&format!("{op:?}"), s);
            all.hist.add(&s.hist).unwrap();
            all.allocs += s.allocs;
        }
    }
    row("ALL", &all);

    let runs: Vec<f64> = (0..5).map(|_| fast_throughput(cfg, &stream)).collect();
    let mut sorted = runs.clone();
    sorted.sort_by(f64::total_cmp);
    println!(
        "throughput (no per-call timer, median of 5): {:.1} ns/op = {:.1} M commands/s",
        sorted[2],
        1_000.0 / sorted[2]
    );

    compare_with_legacy(1_000_000);
}
