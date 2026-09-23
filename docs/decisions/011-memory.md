# 011 — Memory, slot layout, capacity

## Slot
```rust
#[repr(C, align(32))]
struct Slot {
    generation: u32, // odd = live, even = free; part of the ID
    prev: u32,
    next: u32,
    remaining: u32,
    filled: u32,     // for total-size reduce/replace (007)
    account: u32,    // STP + ownership (006)
    price: u16,
    side: u8,
    _pad: u8,
}                    // 28 bytes of fields + 4 bytes alignment padding (spare for future use)
const _: () = assert!(std::mem::size_of::<Slot>() == 32);
```
- Four slots per 128-byte cache line (Apple Silicon); none straddles a line.
- Odd/even occupancy halves the generation range to 2³¹ uses per slot (numbers in 002).
- No `req` (resting orders are identified by `id`). No hot/cold split: matching touches
  next/remaining/filled/account, cancel needs the rest; at 32 bytes a split adds accesses.
  Revisit only with profiling.
- All slots allocated and **prefaulted** at startup. Capacity N is fixed; N < 2³² (001).

## When full (free list empty)
- Post-only, or non-marketable GTC → `Rejected { BookFull }` before touching the book.
- Marketable GTC → match normally; allocate the slot only when a residual must rest.
  Proof: a marketable order with a residual has fully consumed ≥ 1 maker (a partial maker fill
  means the incoming order is done; STP cancels rather than rests), and that maker's slot is freed.
- **Edge case:** if that consumed maker's slot *retires* (002) instead of being freed, the
  residual is cancelled with `Cancelled { reason: CapacityExhausted }`.
  A tiny-capacity, tiny-generation property test must force this path.

## Invariant
An accepted GTC is never downgraded because ordinary capacity is exhausted; it can lose a
residual only through the explicit `CapacityExhausted` retirement edge case.
Also: `live_slots + free_slots + retired_slots == N`; allocations per command == 0.
