# 009 — Price levels and best price

## Decision
- Per side, a fixed array `[Level; N_TICKS]` indexed by price. No tree, no hashing, no allocation.
- `Level { head: u32, tail: u32, total: u64 }`. No `count`: nothing in matching needs it.
- Per side, an occupancy bitset `[u64; 2]` (99 prices). Best bid = highest set bit, best ask =
  lowest. Constant time and branch-light; **not claimed as one instruction** — performance is
  asserted only after measurement. For 999 ticks: 16 words + a summary word.

## Funnel rule
Only three functions write level state:
| Helper | Does | Never |
|---|---|---|
| `level_push(side, price, slot)` | append at tail, set bit if level was empty | — |
| `level_remove(side, price, slot)` | unlink, clear bit if level became empty | — |
| `level_sub_qty(side, price, qty)` | `total -= qty` (partial fill, partial reduce) | unlink, touch bit |

Every command (rest, post-only rest, fill, cancel, reduce, replace, future ones) goes through them,
so the bitset cannot be forgotten.

## Invariants
- `head == NIL ⇔ tail == NIL ⇔ total == 0 ⇔ bit clear`
- `level.total == Σ remaining of linked slots`
- Book never crossed: `best_bid < best_ask` when both exist.
