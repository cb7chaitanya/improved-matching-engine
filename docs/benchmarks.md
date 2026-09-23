# Benchmarks

`cargo run --release --bin latency [ops]` — Apple M3, macOS, one P-core requested via QoS.

## What is measured

**Service time** of `FastBook::process`: one command, engine only, closed loop. This is not
response time under load (queueing), which needs an open-loop generator and a pipeline in
front of the engine.

1. **Record**: the synthetic binary-market workload (`src/workload.rs`) drives the engine;
   every envelope is recorded with its operation label.
2. **Replay ×5**: each run feeds the recorded stream to a fresh engine, timing each call.
   Determinism (E6) makes every replay behave identically. Tables use the **combined**
   distribution of all 5 runs, never the best run. The first 5% of each run is excluded.
3. **Throughput**: replay with no per-call timer; median of 5 runs. This mean is exact.

Workload: 2M commands, ~5,000 resting orders, prices 1–99. Roughly 38% new GTC, 9% post-only,
5% IOC, 3% market, 22%+ cancel, 5% reduce, 5% replace.

## Results (2026-09-23)

| op | p50 | p90 | p99 | p99.9 | p99.99 | allocs/op |
|---|---|---|---|---|---|---|
| GTC rest | <42 | 42 | 42 | 42 | 708 | 0 |
| GTC match | 41 | 42 | 83 | 84 | 708 | 0 |
| IOC | 41 | 42 | 83 | 84 | 708 | 0 |
| Market | 41 | 42 | 84 | 125 | 709 | 0 |
| Post-only rest | <42 | 42 | 42 | 42 | 667 | 0 |
| Post-only reject | <42 | 42 | 42 | 42 | 625 | 0 |
| Cancel | <42 | 42 | 42 | 42 | 667 | 0 |
| Reduce | <42 | 42 | 42 | 42 | 667 | 0 |
| Replace | 41 | 42 | 42 | 83 | 708 | 0 |
| **All** | **<42** | **42** | **42** | **84** | **708** | **0** |

**Mean: 18.4 ns/command (~54M commands/s on one core).**

Per-run p99.9: 625, 84, 84, 84, 84 ns; an earlier run of the same code gave 750, 166, 84, 84, 84 ns (see "Tail variance" below).

### Against the original engine (github.com/cb7chaitanya/matching-engine @ 1934d9e)

Same 1M GTC-only orders (the original has no other order types or cancels), original code
copied verbatim into `src/bin/latency/legacy.rs`:

| | mean | p50 | p99 | p99.9 | p99.99 | max | allocs/op |
|---|---|---|---|---|---|---|---|
| original | 176.2 ns | 84 | 375 | 959 | 5,127 | 511 µs | 2.50 |
| fast | 12.7 ns | <42 | 42 | 167 | 334 | 7.3 µs | 0 |

**14× lower mean, ~9× lower p99, ~6× lower p99.9, ~70× lower max.** Excludes everything the original did
outside `process_order` (Redis, JSON, per-order O(book) `snapshot()`), which dominated its
real per-order cost.

## How to read these numbers

- **Timer resolution.** Apple Silicon's timer ticks at 24 MHz: every single-call timing is a
  multiple of 41.67 ns, and `<42` means "finished within one tick". Percentiles below ~100 ns
  are therefore coarse; the 18.4 ns throughput mean is the precise figure. On Linux x86,
  `rdtsc` gives sub-ns resolution.
- **p99.99 of ~0.6–2 µs** (varies by run) is the same for every operation type, including a post-only reject
  that does almost nothing. That points at the machine (interrupts, scheduler), not the
  engine. It cannot be removed on macOS, which cannot isolate a core.

## Tail variance: investigation

The first benchmark version reported p99.9 ≈ 790 ns for every operation type. Suspicious,
because a cancel and a market sweep do very different work. Checked, in order:

| Hypothesis | Test | Result |
|---|---|---|
| Timer noise | Time an empty call the same way | p99.9 = 42 ns → **not the timer** |
| Page faults | `getrusage` minor faults during replay | 1–2 per 2M calls → **no** |
| CPU frequency / E-core | 2 s busy spin first; compare means | Means identical (~46 ns) → **no** |
| Input stream cold | Read the 96 MB stream once before timing | No change → **no** |
| Warm-up transient | Where in the run do slow calls occur? | Spread evenly through the whole run → **no** |
| Memory placement | Shift addresses with padding allocations | Tail vanished (p99.9 = 84 ns in all 7 runs) |

Conclusion: the ~600–800 ns p99.9 appears in some runs and not others, on identical input and
code, and moves with memory placement and OS state. It is environmental. Probable mechanism:
cache-set or TLB conflicts between the engine's arrays at particular addresses. Confirming it
needs hardware counters (`perf` on Linux). The benchmark therefore reports all runs combined
plus the per-run spread, so a lucky run cannot hide a bad one.

## Not yet measured

- Response time under open-loop load (queueing, coordinated omission): needs the pipeline.
- Burst scenario: correlated cancel/replace across many books.
- LIFO vs FIFO free list (decision 010): cache misses per fill, on Linux with `perf`.
- Linux with an isolated, pinned core: the numbers to quote externally.
