mod common;

use std::collections::HashSet;

use common::{Profile, Step, cents, envelope, record_issued, step, tiny};
use omnibook::audit::Audit;
use omnibook::reference::Reference;
use omnibook::types::*;
use proptest::prelude::*;
use proptest::strategy::ValueTree;
use proptest::test_runner::TestRunner;

fn run(cfg: Config, steps: &[Step]) -> Result<Vec<Event>, String> {
    let mut engine = Reference::new(cfg);
    let mut audit = Audit::new(cfg);
    let mut issued = Vec::new();
    let mut out = Vec::new();
    let mut all = Vec::new();
    for (n, s) in steps.iter().enumerate() {
        let env = envelope(n, s, &issued);
        let before = engine.clone();
        out.clear();
        engine.process(&env, &mut out);
        audit
            .check(&env, &out)
            .map_err(|e| format!("step {n}: {e}"))?;
        engine.check_state().map_err(|e| format!("step {n}: {e}"))?;
        if let [Event::Rejected { .. }] = out.as_slice()
            && before != engine
        {
            return Err(format!("step {n}: E5: rejection changed state"));
        }
        if engine.live_orders() != audit.live_orders() {
            return Err(format!(
                "step {n}: engine and audit disagree on live order count"
            ));
        }
        record_issued(&mut issued, &out);
        all.extend_from_slice(&out);
    }
    Ok(all)
}

fn check_profile(p: Profile, steps: Vec<Step>) -> Result<(), TestCaseError> {
    let first = run(p.cfg, &steps).map_err(TestCaseError::fail)?;
    let second = run(p.cfg, &steps).map_err(TestCaseError::fail)?;
    prop_assert_eq!(first, second, "E6: non-deterministic");
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

    #[test]
    fn tiny_book_invariants(steps in proptest::collection::vec(step(&tiny()), 1..200)) {
        check_profile(tiny(), steps)?;
    }

    #[test]
    fn cents_book_invariants(steps in proptest::collection::vec(step(&cents()), 1..300)) {
        check_profile(cents(), steps)?;
    }
}

#[test]
fn generators_reach_every_outcome() {
    let mut seen: HashSet<String> = HashSet::new();
    let mut runner = TestRunner::deterministic();
    for profile in [tiny, cents] {
        let p = profile();
        let strategy = proptest::collection::vec(step(&p), 50..200);
        for _ in 0..300 {
            let steps = strategy.new_tree(&mut runner).unwrap().current();
            for e in run(p.cfg, &steps).unwrap() {
                let label = match e {
                    Event::Rejected { reason, .. } => format!("Rejected::{reason:?}"),
                    Event::Cancelled { reason, .. } => format!("Cancelled::{reason:?}"),
                    other => format!("{other:?}")
                        .split([' ', '{'])
                        .next()
                        .unwrap()
                        .to_string(),
                };
                seen.insert(label);
            }
        }
    }
    let expected = [
        "Trade",
        "MakerFilled",
        "Rested",
        "Filled",
        "Reduced",
        "Rejected::InvalidPrice",
        "Rejected::InvalidQty",
        "Rejected::PostOnlyWouldCross",
        "Rejected::BookFull",
        "Rejected::UnknownOrStaleId",
        "Rejected::InvalidChange",
        "Cancelled::User",
        "Cancelled::Unfilled",
        "Cancelled::SelfTrade",
        "Cancelled::ReduceToZero",
        "Cancelled::Replaced",
        "Cancelled::CapacityExhausted",
    ];
    let missing: Vec<_> = expected.iter().filter(|l| !seen.contains(**l)).collect();
    assert!(missing.is_empty(), "generators never produced: {missing:?}");
}
