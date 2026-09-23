# 012 — Engine boundary

## Decision
The core is `(book, command) → (book', events)`:
- **Single thread**; no locks, no atomics.
- **No clock.** The serialized ingress stamps each command envelope `{ req, ts, account, body }`;
  the engine copies `ts` into events. Timestamps are **data, never priority**: ordering comes only
  from position in the input sequence, so a clock stepping backwards cannot affect matching.
- **No I/O, no logging.** Events are the only output.
- **No allocation** after startup (011).

## Failure policy: fail closed
A detected invariant violation means the book is untrustworthy: halt, fence trading, never log and
continue. `panic = "abort"` in the release profile so no half-updated state is unwound through.

Two tiers of checks:
- **Production:** O(1) local asserts at the point of change (no underflow, bit set when popping,
  slot live when unlinking).
- **Tests and a shadow replica:** the full O(book) checker after every command.

Determinism cuts both ways: a standby replaying the same input hits the same violation at the same
command. Failover protects against machine failure, not code failure. Recovery from a logic bug =
locate the poison command in the journal, fix, replay.
