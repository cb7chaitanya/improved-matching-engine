# 001 — Numeric representation

## Context
Prices and quantities must compare and add exactly. Binary floating point cannot
represent most decimal prices (`0.1 + 0.2 != 0.3`), so equality and accumulation break.

## Decision
- `Price(u16)` — a **tick count**, not a currency amount. Tick size is instrument config.
  For a 1¢ binary contract the valid range is `1..=99`; `0` and `100` are invalid (a
  contract at $0 or $1 is already certain). A 0.1¢ tick gives `1..=999` with no engine change.
- `Qty(u32)` — lots. Valid range `1..=max_order_qty` (instrument config; also a fat-finger guard).
- Level total: `u64`. Notional (`price × qty`), if ever computed: `u64`.
- `Price` and `Qty` are distinct newtypes so the compiler rejects mixing them.
- Validation happens **inside the engine**, even if the gateway also validates. Invalid
  input produces `Rejected { req, reason: InvalidPrice | InvalidQty }`, never a panic.

## Overflow bounds
| Value | Type | Bound and why |
|---|---|---|
| order remaining / filled | `u32` | ≤ original qty; fills subtract `min(taker, maker)`, so no underflow |
| level total | `u64` | ≤ N × u32::MAX. With N < 2³² this is < 2⁶⁴. At N = 10⁶: ≈ 4.3 × 10¹⁵ vs 1.8 × 10¹⁹ |
| notional | `u64` | u16 × u32 < 2⁴⁸ |

The level-total proof **depends on capacity N < 2³²** (see 011). Raising capacity
requires redoing it.

## Consequences
- Hot-path arithmetic is plain `+`/`-` with `debug_assert!`; no checked-arithmetic branches.
- Unit-mixing bugs are compile errors.
