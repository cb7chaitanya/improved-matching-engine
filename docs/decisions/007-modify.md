# 007 — Reduce and replace

All client quantities for an existing order mean **total size = filled + remaining**. Every live
slot stores `filled: u32`. Rationale: a client acting on a stale view (fills in flight) can never
end up with more exposure than requested, and retries are idempotent.

**Idempotent means same end state, not same response.** A retried reduce that already cancelled
the order gets `UnknownOrStaleId`; that is correct.

## Reduce { req, account, id, total_qty } — keeps queue priority
After the lookup check (002):
| Condition | Result |
|---|---|
| `total_qty <= filled` | cancel remainder → `Cancelled { reason: ReduceToZero }` (past fills stand) |
| `filled < total_qty < filled + remaining` | `remaining = total_qty - filled` → `Reduced { id, filled, remaining }` |
| `total_qty == filled + remaining` | no-op → `Reduced { id, filled, remaining }` |
| `total_qty > filled + remaining` | `Rejected { reason: InvalidChange }` (increases require Replace) |

## Replace { req, account, id, price, total_qty, post_only } — loses priority, new ID
Side is retained. **All-or-nothing**: every possible rejection is decided while the old order is live.
1. Static validation (`price` in range, then `total_qty <= max_order_qty`; `total_qty == 0` is
   valid and means cancel), then the lookup check (002). Failure → `Rejected`, old untouched.
2. `new_remaining = total_qty - old.filled`. If ≤ 0 → atomic cancel of the old order
   (`Cancelled { reason: Replaced }`), no new order, no further checks.
3. If `post_only`: marketability against the opposite side. Removing the old order only removes
   same-side liquidity, so it cannot change this result. Crossing → `Rejected`, old untouched.
4. Capacity: removal frees one slot and the replacement needs at most one, so this is net zero —
   **except** when the old slot will retire on free (002) and the free list is empty. That is
   decidable here → `Rejected { reason: BookFull }`, old untouched.
5. Commit: `Cancelled { old_id, reason: Replaced }`, then the replacement's normal New sequence
   (it may trade, rest, or be cancelled by STP — outcomes, not rejections). New ID, `filled = 0`.

Replacement kinds: GTC limit or post-only. IOC replacement is not supported.
