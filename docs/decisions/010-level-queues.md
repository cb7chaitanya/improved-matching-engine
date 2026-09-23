# 010 — Orders within a level

## Decision
Intrusive doubly linked list through the slots, using `u32` slot indices (`NIL = u32::MAX`).
O(1) append, pop-front, and middle removal; cancelled slots return to the free list immediately.

## Rejected alternatives
- `VecDeque` per level: O(n) middle cancel, allocation on growth.
- Singly linked: middle removal needs the predecessor → O(n).
- **Tombstones** (mark dead, skip later): shifts the cancel's cost onto an unrelated future
  order — after 10,000 cancels a 1-lot order may traverse 10,000 dead entries, breaking the work
  bound and p99. Dead slots also can't be reused until swept, so dead orders can exhaust capacity.

## Rules
- Indices remove raw-pointer lifetime bugs, **not** stale-link logic bugs. The level helpers (009)
  are the sole writers of `prev`/`next`.
- A slot is fully detached (links cleared) before it returns to the free list.

## Open: LIFO vs FIFO free list
LIFO warms allocation, but queue-sweep locality depends on when each linked slot was allocated.
Measure both with quote-cancel bursts and market sweeps: cycles per command, p99, cache misses per
fill (miss counters on Linux `perf`; macOS Instruments CPU Counters is limited).
