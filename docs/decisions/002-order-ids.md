# 002 — Order identity

## Context
Cancel, reduce and replace must find a resting order in O(1) without hashing or allocation,
and a stale ID must never affect a different, newer order.

## Decision
- **Engine-assigned, opaque IDs**: `OrderId(u64) = (generation: u32) << 32 | (slot: u32)`.
  The slot is the order's index in the preallocated slot array (011); lookup is `slots[slot]`.
- The generation is stored **in the slot** and survives while the slot is free.
  Odd = live, even = free (011), so each slot has 2³¹ usable generations.
- **Only orders that rest receive an ID.** IOC/market orders never rest (004) and take no slot.
- Every command carries an opaque `req: u64` assigned by the gateway and echoed on every event
  it causes. That is how a client correlates rejections and learns its new ID (`Rested{req,id}`).
  Client-chosen order IDs are mapped to engine IDs by the gateway, outside the core.
- **Free list: LIFO** (recently freed slots are warm in cache). Revisit with measurement (010).
- **Retire, never wrap**: when freeing a slot whose generation cannot advance without wrapping,
  the slot is retired permanently instead of returning to the free list. Expose a
  `retired_slots` metric.

## Lookup check (cancel / reduce / replace), in order
1. `slot < capacity` — the ID is untrusted input; out-of-range is a rejection, never a panic.
2. slot is live (odd generation).
3. `slot.generation == id.generation`.
4. `slot.account == cmd.account` (006).

Any failure → `Rejected { req, reason: UnknownOrStaleId }` with no state change.
Wrong owner is deliberately indistinguishable from unknown, so IDs cannot be probed.

## Lifetime numbers (5M orders/s sustained)
- Worst case, one hot slot reused on every order: 2³¹ / 5×10⁶ ≈ 430 s → that slot retires in ~7 min.
- Total allocations before all slots retire: N × 2³¹, independent of LIFO/FIFO.
  N = 10⁶ → ≈ 13.6 years. Engines restart far sooner.

## Invariants
- An ID never refers to two different orders over the engine's lifetime.
- IDs are a deterministic function of the input sequence (replay reproduces them).
