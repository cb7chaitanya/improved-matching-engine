# 008 — Event contract

## Identification
An order is identified by `req` until it rests, and by `id` afterwards.

## Sequences
```
New      → Rejected{req, reason}
         | ( Trade{taker_req, maker_id, price, qty, maker_remaining, ts}
             [ MakerFilled{id}  immediately after, iff maker_remaining == 0 ] )*
           then exactly one of: Rested{req, id, qty} | Cancelled{req, qty, reason} | Filled{req}
Cancel   → Cancelled{req, id, qty, reason: User} | Rejected{req, reason}
Reduce   → Reduced{req, id, filled, remaining} | Cancelled{req, id, qty, reason: ReduceToZero}
         | Rejected{req, reason}
Replace  → Rejected{req, reason}
         | Cancelled{req, old_id, qty, reason: Replaced} then the replacement's New sequence
```
One `Rejected { req, reason }` event is used for every command type; it always means
**no state change**. (Proposed simplification of the earlier separate `CancelRejected`.)

## Reasons
- Rejected: `InvalidPrice`, `InvalidQty`, `PostOnlyWouldCross`, `BookFull`,
  `UnknownOrStaleId` (unknown, stale generation, or wrong owner), `InvalidChange`.
- Cancelled: `User`, `Unfilled` (IOC/market remainder), `SelfTrade`, `ReduceToZero`,
  `Replaced`, `CapacityExhausted` (011).

## Rules
- `Trade.price` is always the maker's resting price.
- `ts` is copied from the command envelope (012); never read from a clock.
- `maker_remaining` is state information; `MakerFilled` is the maker's terminal event.
- Events are emitted into a caller-provided `EventSink`. The engine never allocates for
  events; buffering (and any growth) is the sink's concern. `Vec<Event>` is a sink for tests.
  An earlier design required the caller to reserve a worst-case `2N+3` buffer; the sink removes that.

## Invariants
- Conservation per New: Σ trade qty + terminal qty == order qty.
- Every order that becomes live gets exactly one terminal event (`Filled`/`MakerFilled`/`Cancelled`).
- After its terminal event, an ID appears in no further event except a `Rejected` for a late command.
