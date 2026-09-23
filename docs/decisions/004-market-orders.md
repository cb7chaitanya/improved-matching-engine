# 004 — Market orders

## Decision
- Market orders are normalized to **IOC limits** so there is one matching path:
  - buy → IOC limit at `protection` if given, else `max_tick` (99)
  - sell → IOC limit at `protection` if given, else `min_tick` (1)
- `protection` is an **absolute `Price`**, validated like any price. Not an offset from best:
  the client cannot know the book at sequencing time, an offset is undefined on an empty book,
  and converting an offset to a price is the client/gateway's job.
- No eligible liquidity → no trades, then `Cancelled { req, qty: full, reason: Unfilled }`.
  Zero fills is a normal IOC outcome, not a rejection.
- IOC and market orders never rest, so they **take no slot and no ID**. They cannot fail with
  `BookFull`. Trades identify the taker by `req`.
