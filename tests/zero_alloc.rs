use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use omnibook::book::FastBook;
use omnibook::types::*;
use omnibook::workload::Workload;

struct Counting;

thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    static ALLOCS: Cell<u64> = const { Cell::new(0) };
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if COUNTING.with(Cell::get) {
            ALLOCS.with(|a| a.set(a.get() + 1));
        }
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if COUNTING.with(Cell::get) {
            ALLOCS.with(|a| a.set(a.get() + 1));
        }
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

struct UncountedSink(Vec<Event>);

impl EventSink for UncountedSink {
    fn emit(&mut self, event: Event) {
        let was = COUNTING.with(|c| c.replace(false));
        self.0.push(event);
        COUNTING.with(|c| c.set(was));
    }
}

fn allocations_during(ops: usize, cfg: Config, target_live: usize) -> u64 {
    let mut engine = FastBook::new(cfg);
    let mut out = UncountedSink(Vec::new());
    let mut flow = Workload::new(42, target_live);
    let mut allocs = 0;
    for _ in 0..ops {
        let env = flow.next(engine.best_bid(), engine.best_ask());
        out.0.clear();
        ALLOCS.with(|a| a.set(0));
        COUNTING.with(|c| c.set(true));
        engine.process(&env, &mut out);
        COUNTING.with(|c| c.set(false));
        allocs += ALLOCS.with(Cell::get);
        flow.observe(&env, &out.0);
    }
    allocs
}

#[test]
fn fast_engine_never_allocates_after_startup() {
    let cfg = Config {
        max_order_qty: Qty(1_000),
        ..Config::binary_cents(1 << 14)
    };
    assert_eq!(allocations_during(300_000, cfg, 5_000), 0);
}

#[test]
fn full_book_with_retirement_never_allocates() {
    let cfg = Config {
        max_order_qty: Qty(1_000),
        generation_limit: 41,
        ..Config::binary_cents(256)
    };
    assert_eq!(allocations_during(300_000, cfg, 5_000), 0);
}

#[test]
fn counter_detects_allocations() {
    COUNTING.with(|c| c.set(true));
    ALLOCS.with(|a| a.set(0));
    let v: Vec<u64> = Vec::with_capacity(8);
    COUNTING.with(|c| c.set(false));
    drop(v);
    assert_eq!(ALLOCS.with(Cell::get), 1);
}
