mod common;

use common::{Profile, Step, cents, envelope, record_issued, step, tiny};
use omnibook::audit::Audit;
use omnibook::book::FastBook;
use omnibook::reference::Reference;
use omnibook::types::*;
use omnibook::workload::Workload;
use proptest::prelude::*;

fn compare(p: Profile, steps: &[Step]) -> Result<(), String> {
    let mut reference = Reference::new(p.cfg);
    let mut fast = FastBook::new(p.cfg);
    let mut audit = Audit::new(p.cfg);
    let mut issued = Vec::new();
    let (mut expected, mut actual) = (Vec::new(), Vec::new());
    for (n, s) in steps.iter().enumerate() {
        let env = envelope(n, s, &issued);
        expected.clear();
        actual.clear();
        reference.process(&env, &mut expected);
        fast.process(&env, &mut actual);
        if expected != actual {
            return Err(format!(
                "step {n}: engines diverge\n  command:   {env:?}\n  reference: {expected:?}\n  fast:      {actual:?}"
            ));
        }
        audit
            .check(&env, &actual)
            .map_err(|e| format!("step {n}: {e}"))?;
        fast.check_invariants()
            .map_err(|e| format!("step {n}: {e}\n  command: {env:?}"))?;
        record_issued(&mut issued, &actual);
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 1024, ..ProptestConfig::default() })]

    #[test]
    fn tiny_book_matches_reference(steps in proptest::collection::vec(step(&tiny()), 1..300)) {
        compare(tiny(), &steps).map_err(TestCaseError::fail)?;
    }

    #[test]
    fn cents_book_matches_reference(steps in proptest::collection::vec(step(&cents()), 1..400)) {
        compare(cents(), &steps).map_err(TestCaseError::fail)?;
    }
}

fn soak(seed: u64, ops: usize, cfg: Config, target_live: usize) {
    let mut reference = Reference::new(cfg);
    let mut fast = FastBook::new(cfg);
    let mut audit = Audit::new(cfg);
    let mut flow = Workload::new(seed, target_live);
    let (mut expected, mut actual) = (Vec::new(), Vec::new());
    for n in 0..ops {
        let env = flow.next(fast.best_bid(), fast.best_ask());
        expected.clear();
        actual.clear();
        reference.process(&env, &mut expected);
        fast.process(&env, &mut actual);
        assert_eq!(expected, actual, "step {n}: engines diverge on {env:?}");
        if let Err(e) = audit.check(&env, &actual) {
            panic!("step {n}: {e}");
        }
        if n % 64 == 0 {
            fast.check_invariants()
                .unwrap_or_else(|e| panic!("step {n}: {e}"));
        }
        flow.observe(&env, &actual);
    }
    fast.check_invariants().unwrap();
}

#[test]
fn soak_realistic_flow() {
    let cfg = Config {
        max_order_qty: Qty(1_000),
        ..Config::binary_cents(4_096)
    };
    for seed in 1..=4 {
        soak(seed, 50_000, cfg, 400);
    }
}

#[test]
fn soak_full_book_with_retirement() {
    let cfg = Config {
        max_order_qty: Qty(1_000),
        generation_limit: 41,
        ..Config::binary_cents(128)
    };
    for seed in 1..=4 {
        soak(seed, 50_000, cfg, 400);
    }
}
