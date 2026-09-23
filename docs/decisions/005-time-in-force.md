# 005 — Time in force

## Decision
Ship **GTC** and **IOC** only.

## FOK deferred
FOK needs a preflight "is there enough liquidity up to my limit?" and must guarantee no partial
execution. It can be added later without changing GTC/IOC semantics.
- The `u64` level totals (001, 009) allow a bounded preflight: sum ≤ 99 level totals.
- **Caveat:** with self-trade prevention (006), level totals overstate fillable liquidity when
  some of it belongs to the same account, so an exact preflight would have to walk orders.
