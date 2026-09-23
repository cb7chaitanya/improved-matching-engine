# 006 — Self-trade prevention and ownership

## Decision
- Every slot stores `account: u32`, an **engine-scoped owner key** assigned by the gateway after
  authentication. It is not an authentication mechanism.
- STP mode: **cancel newest.** When the incoming order reaches a resting order of the same
  account, fills made so far stand, the incoming remainder is cancelled with
  `Cancelled { req, qty, reason: SelfTrade }`, and matching stops — even if other accounts'
  liquidity sits behind it.
- Cancel / reduce / replace check `slot.account == cmd.account` (002, step 4).

## Why not cancel-oldest
Normally makers touched ≤ incoming qty, because each fill consumes ≥ 1 lot. Cancelling own
resting orders consumes no incoming qty, so with cancel-oldest a 1-lot order could cancel
thousands of orders: per-order work would depend on the account's book footprint, an
unbounded p99 source. Cancel-newest keeps **makers touched ≤ qty + 1**.

## Invariants
- An order changes only via its own account's commands or by trading.
- No trade has `taker account == maker account`.
