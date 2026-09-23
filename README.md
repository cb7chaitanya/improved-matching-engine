# omnibook

A single-book matching engine for binary event contracts (prices 1–99¢): limit (GTC/IOC),
market (with absolute protection), post-only, cancel, reduce and replace. Deterministic,
single-threaded, zero allocation after startup.

- **Decisions**: `docs/decisions/001–012` — every rule, with alternatives and reasons.
- **Invariants**: `docs/invariants.md` — 33 numbered invariants and how each is checked.
- **Benchmarks**: `docs/benchmarks.md` — 18.4 ns/command mean, p99 42 ns, 0 allocations.

## Layout

| | |
|---|---|
| `src/types.rs` | Commands, events, config |
| `src/reference.rs` | Slow, obviously correct executable specification |
| `src/book.rs` | Fast engine: level arrays, bitsets, 32-byte slots, intrusive FIFO lists |
| `src/audit.rs` | Independent checker that rebuilds the book from events alone |
| `src/workload.rs` | Deterministic synthetic order flow |
| `tests/scenarios.rs` | One readable example per decision |
| `tests/properties.rs` | Random sequences against the reference, all invariants per command |
| `tests/differential.rs` | Fast engine must emit the reference's exact events |
| `tests/zero_alloc.rs` | Proves no allocation inside `process` |

## Run

```sh
cargo test --release                    # all tests
cargo run --release --bin latency       # benchmark
```
